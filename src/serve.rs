use std::{collections::HashMap, io, net::SocketAddr, path::PathBuf, process::Stdio};

use askama::Template;
use bytes::BytesMut;
use futures_util::stream::TryStreamExt;
use include_dir::{include_dir, Dir};
use thiserror::Error;
use tokio::{
    fs::File,
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{ChildStdout, Command},
};
use tokio_stream::StreamExt;
use tokio_util::codec::{BytesCodec, FramedRead};
use warp::{
    host::Authority,
    http,
    hyper::{body::Sender, Body, Response},
    path::Tail,
    reject::Reject,
    Filter, Rejection, Stream,
};

#[derive(Template)]
#[template(path = "index.html")]
struct IndexTemplate {
    platforms: Vec<(bool, String)>,
    host: String,
}

const STATIC_DIR: Dir = include_dir!("static");

#[derive(Error, Debug)]
pub enum ServeError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
    #[error("Hyper error: {0}")]
    Hyper(#[from] warp::hyper::Error),
    #[error("Warp HTTP error: {0}")]
    Warp(#[from] warp::http::Error),
    #[error("{0}")]
    Other(String),
}

impl Reject for ServeError {}

pub async fn serve(path: PathBuf, socket_addr: SocketAddr, tls_paths: Option<(PathBuf, PathBuf)>) {
    let index_path = path.clone();
    let is_tls = tls_paths.is_some();

    // Handle the homepage
    let index = warp::path::end().and(warp::host::optional()).and_then(
        move |authority: Option<Authority>| {
            let mirror_path = index_path.clone();
            let protocol = if is_tls { "https://" } else { "http://" };
            async move {
                get_rustup_platforms(mirror_path)
                    .await
                    .map(|platforms| IndexTemplate {
                        platforms,
                        host: authority
                            .map(|a| format!("{}{}", protocol, a.as_str()))
                            .unwrap_or_else(|| "http://panamax.internal".to_string()),
                    })
                    .map_err(|_| {
                        warp::reject::custom(ServeError::Other(
                            "Could not retrieve rustup platforms.".to_string(),
                        ))
                    })
            }
        },
    );

    // Handle all files baked into the binary at /static
    let static_dir =
        warp::path::path("static")
            .and(warp::path::tail())
            .and_then(|path: Tail| async move {
                STATIC_DIR
                    .get_file(path.as_str())
                    .ok_or_else(warp::reject::not_found)
                    .map(|f| f.contents().to_vec())
            });

    let dist_dir = warp::path::path("dist").and(warp::fs::dir(path.join("dist")));
    let rustup_dir = warp::path::path("rustup").and(warp::fs::dir(path.join("rustup")));

    // Handle crates requests in the format of "/crates/ripgrep/0.1.0/download"
    let crates_mirror_path = path.clone();
    let crates_dir_native_format = warp::path!("crates" / String / String / "download").and_then(
        move |name: String, version: String| {
            let mirror_path = crates_mirror_path.clone();
            async move { get_crate_file(mirror_path, &name, &version).await }
        },
    );

    // Handle crates requests in the format of "/crates/ripgrep/ripgrep-0.1.0.crate"
    let crates_mirror_path_2 = path.clone();
    let crates_dir_condensed_format = warp::path!("crates" / String / String).and_then(
        move |name: String, crate_file: String| {
            let mirror_path = crates_mirror_path_2.clone();
            async move {
                if !crate_file.ends_with(".crate") || !crate_file.starts_with(&name) {
                    return Err(warp::reject::not_found());
                }
                let version = &crate_file[name.len() + 1..crate_file.len() - 6];
                get_crate_file(mirror_path, &name, &version).await
            }
        },
    );

    // Handle git client requests to /git/crates.io-index
    let git_mirror_path = path.clone();
    let git = warp::path("git")
        .and(warp::path("crates.io-index"))
        .and(warp::path::tail())
        .and(warp::method())
        .and(warp::header::optional::<String>("Content-Type"))
        .and(warp::query::raw())
        .and(warp::addr::remote())
        .and(warp::body::stream())
        .and_then(
            move |path_tail, method, content_type, query, remote, body| {
                let mirror_path = git_mirror_path.clone();
                async move {
                    handle_git(
                        mirror_path,
                        path_tail,
                        method,
                        content_type,
                        query,
                        remote,
                        body,
                    )
                    .await
                }
            },
        );

    let git_nq_mirror_path = path.clone();
    // query::raw() seems to expect a non-empty query string
    // so create a separate set of filters for when it's empty
    let git_no_query = warp::path("git")
        .and(warp::path("crates.io-index"))
        .and(warp::path::tail())
        .and(warp::method())
        .and(warp::header::optional::<String>("Content-Type"))
        .and(warp::addr::remote())
        .and(warp::body::stream())
        .and_then(move |path_tail, method, content_type, remote, body| {
            let mirror_path = git_nq_mirror_path.clone();

            async move {
                handle_git_empty_query(mirror_path, path_tail, method, content_type, remote, body)
                    .await
            }
        });

    let routes = index
        .or(static_dir)
        .or(dist_dir)
        .or(rustup_dir)
        .or(crates_dir_native_format)
        .or(crates_dir_condensed_format)
        .or(git)
        .or(git_no_query);

    match tls_paths {
        Some((cert_path, key_path)) => {
            println!("Running TLS on {}", socket_addr);
            warp::serve(routes)
                .tls()
                .cert_path(cert_path)
                .key_path(key_path)
                .run(socket_addr)
                .await;
        }
        None => {
            println!("Running HTTP on {}", socket_addr);
            warp::serve(routes).run(socket_addr).await;
        }
    }
}

/// Get all rustup platforms available on the mirror.
async fn get_rustup_platforms(path: PathBuf) -> io::Result<Vec<(bool, String)>> {
    let rustup_path = path.join("rustup/dist");

    let mut output = vec![];

    // Look at the rustup/dist directory for all rustup-init and rustup-init.exe files.
    // Also return if the rustup-init file is a .exe or not.
    let mut rd = tokio::fs::read_dir(rustup_path).await?;
    while let Some(entry) = rd.next_entry().await? {
        if entry.metadata().await?.is_dir() {
            if let Some(name) = entry.file_name().to_str() {
                let name = name.to_string();
                if entry.path().join("rustup-init").exists() {
                    output.push((false, name));
                } else if entry.path().join("rustup-init.exe").exists() {
                    output.push((true, name));
                }
            }
        }
    }

    // Sort by name, keeping non-exe versions at the top.
    output.sort();

    Ok(output)
}

/// Return a crate file as an HTTP response.
async fn get_crate_file(
    mirror_path: PathBuf,
    name: &str,
    version: &str,
) -> Result<Response<Body>, Rejection> {
    let crate_path = match name.len() {
        1 => PathBuf::from("1"),
        2 => PathBuf::from("2"),
        3 => PathBuf::from("3"),
        n if n >= 4 => {
            let first_two = name.get(0..2).ok_or_else(warp::reject::not_found)?;
            let second_two = name.get(2..4).ok_or_else(warp::reject::not_found)?;

            [first_two, second_two].iter().collect()
        }
        _ => return Err(warp::reject::not_found()),
    };

    let full_path = mirror_path
        .join("crates")
        .join(crate_path)
        .join(name)
        .join(version)
        .join(format!("{}-{}.crate", name, version));

    let file = File::open(full_path)
        .await
        .map_err(|_| warp::reject::not_found())?;
    let meta = file
        .metadata()
        .await
        .map_err(|_| warp::reject::not_found())?;
    let stream = FramedRead::new(file, BytesCodec::new()).map_ok(BytesMut::freeze);

    let body = Body::wrap_stream(stream);

    let mut resp = Response::new(body);
    resp.headers_mut()
        .insert(http::header::CONTENT_LENGTH, meta.len().into());

    Ok(resp)
}

/// Handle a request from a git client.
/// Special case for empty query strings.
async fn handle_git_empty_query<S, B>(
    mirror_path: PathBuf,
    path_tail: Tail,
    method: http::Method,
    content_type: Option<String>,
    remote: Option<SocketAddr>,
    body: S,
) -> Result<Response<Body>, Rejection>
where
    S: Stream<Item = Result<B, warp::Error>> + Send + Unpin + 'static,
    B: bytes::Buf + Sized,
{
    handle_git(
        mirror_path,
        path_tail,
        method,
        content_type,
        String::new(),
        remote,
        body,
    )
    .await
}

/// Handle a request from a git client.
async fn handle_git<S, B>(
    mirror_path: PathBuf,
    path_tail: Tail,
    method: http::Method,
    content_type: Option<String>,
    query: String,
    remote: Option<SocketAddr>,
    mut body: S,
) -> Result<Response<Body>, Rejection>
where
    S: Stream<Item = Result<B, warp::Error>> + Send + Unpin + 'static,
    B: bytes::Buf + Sized,
{
    let remote = remote
        .map(|r| r.ip().to_string())
        .unwrap_or_else(|| "127.0.0.1".to_string());

    // Run "git http-backend"
    let mut cmd = Command::new("git");
    cmd.arg("http-backend");

    // Clear environment variables, and set needed variables
    // See: https://git-scm.com/docs/git-http-backend
    cmd.env_clear();
    cmd.env("GIT_PROJECT_ROOT", mirror_path);
    cmd.env(
        "PATH_INFO",
        format!("/crates.io-index/{}", path_tail.as_str()),
    );
    cmd.env("REQUEST_METHOD", method.as_str());
    cmd.env("QUERY_STRING", query);
    cmd.env("REMOTE_USER", "");
    cmd.env("REMOTE_ADDR", remote);
    if let Some(content_type) = content_type {
        cmd.env("CONTENT_TYPE", content_type);
    }
    cmd.env("GIT_HTTP_EXPORT_ALL", "true");
    cmd.stderr(Stdio::inherit());
    cmd.stdout(Stdio::piped());
    cmd.stdin(Stdio::piped());

    let p = cmd.spawn().map_err(ServeError::from)?;

    // Handle sending git client body to http-backend, if any
    let mut git_input = p.stdin.expect("Process should always have stdin");
    while let Some(Ok(mut buf)) = body.next().await {
        git_input
            .write_all_buf(&mut buf)
            .await
            .map_err(ServeError::from)?;
    }

    // Collect headers from git CGI output
    let mut git_output = BufReader::new(p.stdout.expect("Process should always have stdout"));
    let mut headers = HashMap::new();
    loop {
        let mut line = String::new();
        git_output
            .read_line(&mut line)
            .await
            .map_err(ServeError::from)?;

        let line = line.trim_end();
        if line.is_empty() {
            break;
        }

        if let Some((key, value)) = line.split_once(": ") {
            headers.insert(key.to_string(), value.to_string());
        }
    }

    // Add headers to response (except for Status, which is the "200 OK" line)
    let mut resp = Response::builder();
    for (key, val) in headers {
        if key == "Status" {
            resp = resp.status(&val.as_bytes()[..3]);
        } else {
            resp = resp.header(&key, val);
        }
    }

    // Create channel, so data can be streamed without being fully loaded
    // into memory. Requires a separate future to be spawned.
    let (sender, body) = Body::channel();
    tokio::spawn(send_git(sender, git_output));

    let resp = resp.body(body).map_err(ServeError::from)?;
    Ok(resp)
}

/// Send data from git CGI process to hyper Sender, until there is no more
/// data left.
async fn send_git(
    mut sender: Sender,
    mut git_output: BufReader<ChildStdout>,
) -> Result<(), ServeError> {
    loop {
        let mut bytes_out = BytesMut::new();
        git_output.read_buf(&mut bytes_out).await?;
        if bytes_out.is_empty() {
            return Ok(());
        }
        sender.send_data(bytes_out.freeze()).await?;
    }
}
