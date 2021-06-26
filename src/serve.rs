use std::{
    collections::HashMap,
    io,
    net::SocketAddr,
    path::{Path, PathBuf},
    process::Stdio,
};

use askama::Template;
use bytes::BytesMut;
use include_dir::{include_dir, Dir};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{ChildStdout, Command},
};
use tokio_stream::StreamExt;
use warp::{
    http,
    hyper::{body::Sender, Body, Response},
    path::Tail,
    Filter, Rejection, Stream,
};

#[derive(Template)]
#[template(path = "index.html")]
struct IndexTemplate {
    platforms: Vec<String>,
    host: String,
}

const STATIC_DIR: Dir = include_dir!("static");

pub async fn serve(path: PathBuf, socket_addr: SocketAddr, tls_paths: Option<(PathBuf, PathBuf)>) {
    let host = "http://panamax.somewhere".to_string();

    let platforms = get_rustup_platforms(&path).await.unwrap();

    let index = warp::path::end().map(move || IndexTemplate {
        platforms: platforms.clone(),
        host: host.clone(),
    });

    let static_dir =
        warp::path::path("static")
            .and(warp::path::tail())
            .and_then(|path: Tail| async move {
                STATIC_DIR
                    .get_file(path.as_str())
                    .ok_or(warp::reject::not_found())
                    .map(|f| f.contents().to_vec())
            });

    let dist_dir = warp::path::path("dist").and(warp::fs::dir(path.join("dist")));
    let rustup_dir = warp::path::path("rustup").and(warp::fs::dir(path.join("rustup")));
    // TODO: crates_dir needs to be translated
    let crates_dir = warp::path::path("crates").and(warp::fs::dir(path.join("crates")));

    let git_mirror_path = path.clone();
    let git = warp::path("git")
        .and(warp::path("crates.io-index"))
        .and(warp::path::tail())
        .and(warp::method())
        .and(warp::header::optional::<String>("Content-Type"))
        .and(warp::header::optional::<String>("Content-Encoding"))
        .and(warp::query::raw())
        .and(warp::addr::remote())
        .and(warp::body::stream())
        .and_then(
            move |path_tail, method, content_type, encoding, query, remote, body| {
                let mirror_path = git_mirror_path.clone();
                async move {
                    handle_git(
                        mirror_path,
                        path_tail,
                        method,
                        content_type,
                        encoding,
                        query,
                        remote,
                        body,
                    ).await
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
        .and(warp::header::optional::<String>("Content-Encoding"))
        .and(warp::addr::remote())
        .and(warp::body::stream())
        .and_then(
            move |path_tail, method, content_type, encoding, remote, body| {
                let mirror_path = git_nq_mirror_path.clone();

                async move {
                    handle_git_empty_query(
                        mirror_path,
                        path_tail,
                        method,
                        content_type,
                        encoding,
                        remote,
                        body,
                    ).await
                }
            },
        );

    let routes = index
        .or(static_dir)
        .or(dist_dir)
        .or(rustup_dir)
        .or(crates_dir)
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
async fn get_rustup_platforms(path: &Path) -> io::Result<Vec<String>> {
    let rustup_path = path.join("rustup/dist");

    let mut output = vec![];

    let mut rd = tokio::fs::read_dir(rustup_path).await?;
    while let Some(entry) = rd.next_entry().await? {
        if entry.metadata().await?.is_dir() {
            if let Some(name) = entry.file_name().to_str() {
                output.push(name.to_string());
            }
        }
    }

    Ok(output)
}

/// Handle a request from a git client.
/// Special case for empty query strings.
async fn handle_git_empty_query<S, B>(
    mirror_path: PathBuf,
    path_tail: Tail,
    method: http::Method,
    content_type: Option<String>,
    encoding: Option<String>,
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
        encoding,
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
    encoding: Option<String>,
    query: String,
    remote: Option<SocketAddr>,
    mut body: S,
) -> Result<Response<Body>, Rejection>
where
    S: Stream<Item = Result<B, warp::Error>> + Send + Unpin + 'static,
    B: bytes::Buf + Sized,
{
    dbg!(
        &path_tail,
        &method,
        &content_type,
        &encoding,
        &query,
        &remote
    );

    let remote = remote
        .map(|r| r.ip().to_string())
        .unwrap_or_else(|| "127.0.0.1".to_string());

    let mut cmd = Command::new("git");
    cmd.arg("http-backend");
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

    let p = cmd.spawn().unwrap();

    let mut git_input = p.stdin.unwrap();
    // Handle sending body to http-backend, if any
    while let Some(Ok(mut buf)) = body.next().await {
        git_input.write_all_buf(&mut buf).await.unwrap();
    }

    let mut git_output = BufReader::new(p.stdout.unwrap());

    // Collect headers from git CGI output
    let mut headers = HashMap::new();
    loop {
        let mut line = String::new();
        git_output.read_line(&mut line).await.unwrap();

        let line = line.trim_end();
        if line.is_empty() {
            break;
        }

        if let Some((key, value)) = line.split_once(": ") {
            headers.insert(key.to_string(), value.to_string());
        }
    }
    dbg!(&headers);

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

    let resp = resp.body(body).unwrap();
    Ok(resp)
}

/// Send data from git CGI process to hyper Sender, until there is no more
/// data left.
async fn send_git(mut sender: Sender, mut git_output: BufReader<ChildStdout>) {
    loop {
        let mut bytes_out = BytesMut::new();
        git_output.read_buf(&mut bytes_out).await.unwrap();
        if bytes_out.is_empty() {
            return;
        }
        sender.send_data(bytes_out.freeze()).await.unwrap();
    }
}
