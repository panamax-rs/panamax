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

use crate::crates::get_crate_path;

pub struct TlsConfig {
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
pub struct Platform {
    is_exe: bool,
    platform_triple: String,
}

#[derive(Template)]
#[template(path = "index.html")]
struct IndexTemplate {
    platforms: Vec<Platform>,
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

pub async fn serve(path: PathBuf, socket_addr: SocketAddr, tls_paths: Option<TlsConfig>) {
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

    // Handle all files baked into the binary with include_dir, at /static
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
    // This format is the default for cargo, and will be used if an external process rewrites config.json in crates.io-index
    let crates_mirror_path = path.clone();
    let crates_dir_native_format = warp::path!("crates" / String / String / "download").and_then(
        move |name: String, version: String| {
            let mirror_path = crates_mirror_path.clone();
            async move { get_crate_file(mirror_path, &name, &version).await }
        },
    );

    // Handle crates requests in the format of either :
    // - "/crates/1/u/0.2.0/u-0.2.0.crate"
    // - "/crates/2/bm/0.11.0/bm-0.11.0.crate"
    // - "/crates/3/c/cde/0.1.1/cde-0.1.1.crate"
    // - "/crates/se/rd/serde/1.0.130/serde-1.0.130.crate"
    // This format is used by Panamax, and/or is used if config.json contains "/crates/{prefix}/{crate}/{version}/{crate}-{version}.crate"
    let crates_mirror_path_2 = path.clone();
    let crates_dir_condensed_format_1 = warp::path!("crates" / "1" / String / String / String)
        .map(|name: String, version: String, crate_file: String| (name, version, crate_file))
        .untuple_one();
    let crates_dir_condensed_format_2 = warp::path!("crates" / "2" / String / String / String)
        .map(|name: String, version: String, crate_file: String| (name, version, crate_file))
        .untuple_one();
    let crates_dir_condensed_format_3 =
        warp::path!("crates" / "3" / String / String / String / String)
            .map(
                |_: String, name: String, version: String, crate_file: String| {
                    (name, version, crate_file)
                },
            )
            .untuple_one();
    let crates_dir_condensed_format_full =
        warp::path!("crates" / String / String / String / String / String)
            .map(
                |_: String, _: String, name: String, version: String, crate_file: String| {
                    (name, version, crate_file)
                },
            )
            .untuple_one();

    let crates_dir_condensed_format = crates_dir_condensed_format_1
        .or(crates_dir_condensed_format_2)
        .unify()
        .or(crates_dir_condensed_format_3)
        .unify()
        .or(crates_dir_condensed_format_full)
        .unify()
        .and_then(move |name: String, version: String, crate_file: String| {
            let mirror_path = crates_mirror_path_2.clone();
            async move {
                if !crate_file.ends_with(".crate") || !crate_file.starts_with(&name) {
                    return Err(warp::reject::not_found());
                }
                get_crate_file(mirror_path, &name, &version).await
            }
        });

    // Handle git client requests to /git/crates.io-index
    let path_for_git = path.clone();
    let git = warp::path("git")
        .and(warp::path("crates.io-index"))
        .and(warp::path::tail())
        .and(warp::method())
        .and(warp::header::optional::<String>("Content-Type"))
        .and(warp::addr::remote())
        .and(warp::body::stream())
        .and(warp::query::raw().or_else(|_| async { Ok::<(String,), Rejection>((String::new(),)) }))
        .and_then(
            move |path_tail, method, content_type, remote, body, query| {
                let mirror_path = path_for_git.clone();
                async move {
                    handle_git(
                        mirror_path,
                        path_tail,
                        method,
                        content_type,
                        remote,
                        body,
                        query,
                    )
                    .await
                }
            },
        );

    let routes = index
        .or(static_dir)
        .or(dist_dir)
        .or(rustup_dir)
        .or(crates_dir_native_format)
        .or(crates_dir_condensed_format)
        .or(git);

    match tls_paths {
        Some(TlsConfig {
            cert_path,
            key_path,
        }) => {
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
async fn get_rustup_platforms(path: PathBuf) -> io::Result<Vec<Platform>> {
    let rustup_path = path.join("rustup/dist");

    let mut output = vec![];

    // Look at the rustup/dist directory for all rustup-init and rustup-init.exe files.
    // Also return if the rustup-init file is a .exe or not.
    let mut rd = tokio::fs::read_dir(rustup_path).await?;
    while let Some(entry) = rd.next_entry().await? {
        if entry.metadata().await?.is_dir() {
            if let Some(name) = entry.file_name().to_str() {
                let platform_triple = name.to_string();
                if entry.path().join("rustup-init").exists() {
                    output.push(Platform {
                        is_exe: false,
                        platform_triple,
                    });
                } else if entry.path().join("rustup-init.exe").exists() {
                    output.push(Platform {
                        is_exe: true,
                        platform_triple,
                    });
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
    let full_path =
        get_crate_path(&mirror_path, name, version).ok_or_else(warp::reject::not_found)?;

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
async fn handle_git<S, B>(
    mirror_path: PathBuf,
    path_tail: Tail,
    method: http::Method,
    content_type: Option<String>,
    remote: Option<SocketAddr>,
    mut body: S,
    query: String,
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
