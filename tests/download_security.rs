mod support;

use flate2::{Compression, write::GzEncoder};
use ktk_schedule::{
    model::week_millis,
    workspace::{FILE_LIMIT, Workspace},
};
use std::{io::Write, sync::Arc, time::Duration};
use support::College;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{Notify, Semaphore},
    task::{JoinHandle, JoinSet},
};

#[derive(Clone, Copy)]
enum Body {
    Length,
    Chunked,
    UntilClose,
    Gzip,
    Slow,
}

// Raw HTTP is needed here: wiremock supplies Content-Length for its response bodies.
struct Server {
    url: reqwest::Url,
    body_started: Arc<Notify>,
    task: JoinHandle<()>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Server {
    async fn start(college: &College, body: Body, size: usize) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap())
            .parse()
            .unwrap();
        let upstream = college.server.uri();
        let body_started = Arc::new(Notify::new());
        let signal = body_started.clone();
        let task = tokio::spawn(async move {
            let client = reqwest::Client::builder()
                .no_gzip()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap();
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    connection = listener.accept() => {
                        let (socket, _) = connection.unwrap();
                        let client = client.clone();
                        let upstream = upstream.clone();
                        let signal = signal.clone();
                        connections.spawn(async move {
                            // The downloader deliberately disconnects once its limit is reached.
                            let _ = serve(socket, client, &upstream, body, size, signal).await;
                        });
                    }
                    Some(result) = connections.join_next() => { result.unwrap(); }
                }
            }
        });
        Self {
            url,
            body_started,
            task,
        }
    }

    async fn login(&self) -> Workspace {
        let client = Workspace::login(
            self.url.clone(),
            "test",
            "user",
            "password",
            Arc::new(Semaphore::new(12)),
        )
        .await
        .unwrap();
        let week = week_millis(
            chrono::NaiveDate::from_ymd_opt(2026, 9, 1).unwrap(),
            chrono_tz::Asia::Yekaterinburg,
        )
        .unwrap();
        client.refresh(269, week, false).await.unwrap();
        client
    }
}

async fn serve(
    mut socket: TcpStream,
    client: reqwest::Client,
    upstream: &str,
    body: Body,
    size: usize,
    signal: Arc<Notify>,
) -> anyhow::Result<()> {
    let mut request = Vec::new();
    let end = loop {
        if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
            break end + 4;
        }
        let mut chunk = [0; 4096];
        let n = socket.read(&mut chunk).await?;
        anyhow::ensure!(n > 0 && request.len() < 65536, "invalid test request");
        request.extend_from_slice(&chunk[..n]);
    };
    let headers = String::from_utf8(request[..end].to_vec())?;
    let mut words = headers.lines().next().unwrap().split_whitespace();
    let method: reqwest::Method = words.next().unwrap().parse()?;
    let path = words.next().unwrap();
    if path == "/download/task.pdf" {
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n")
            .await?;
        match body {
            Body::Length => {
                socket
                    .write_all(format!("Content-Length: {size}\r\n\r\n").as_bytes())
                    .await?;
            }
            Body::Chunked => {
                socket
                    .write_all(b"Transfer-Encoding: chunked\r\n\r\n")
                    .await?
            }
            Body::UntilClose => socket.write_all(b"\r\n").await?,
            Body::Gzip => {
                let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
                let chunk = [b'x'; 65536];
                let mut left = size;
                while left > 0 {
                    let n = left.min(chunk.len());
                    encoder.write_all(&chunk[..n])?;
                    left -= n;
                }
                let compressed = encoder.finish()?;
                socket
                    .write_all(
                        format!(
                            "Content-Encoding: gzip\r\nContent-Length: {}\r\n\r\n",
                            compressed.len()
                        )
                        .as_bytes(),
                    )
                    .await?;
                socket.write_all(&compressed).await?;
                return Ok(());
            }
            Body::Slow => {
                socket.write_all(b"Content-Length: 2\r\n\r\nx").await?;
                socket.flush().await?;
                signal.notify_one();
                std::future::pending::<()>().await;
                return Ok(());
            }
        }
        // A lying, oversized Content-Length must be rejected before waiting for a body.
        if matches!(body, Body::Length) && size > FILE_LIMIT {
            socket.flush().await?;
            std::future::pending::<()>().await;
        }
        let chunk = [b'x'; 65536];
        let mut left = size;
        while left > 0 {
            let n = left.min(chunk.len());
            if matches!(body, Body::Chunked) {
                socket.write_all(format!("{n:x}\r\n").as_bytes()).await?;
            }
            socket.write_all(&chunk[..n]).await?;
            if matches!(body, Body::Chunked) {
                socket.write_all(b"\r\n").await?;
            }
            left -= n;
        }
        if matches!(body, Body::Chunked) {
            socket.write_all(b"0\r\n\r\n").await?;
        }
        return Ok(());
    }
    let length: usize = headers
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(key, _)| key.eq_ignore_ascii_case("content-length"))
        .map(|(_, value)| value.trim().parse().unwrap())
        .unwrap_or(0);
    while request.len() < end + length {
        let mut chunk = [0; 4096];
        let n = socket.read(&mut chunk).await?;
        anyhow::ensure!(n > 0, "truncated test request");
        request.extend_from_slice(&chunk[..n]);
    }
    let response = client
        .request(method, format!("{upstream}{path}"))
        .body(request[end..end + length].to_vec())
        .send()
        .await?;
    let mut head = format!(
        "HTTP/1.1 {} OK\r\nConnection: close\r\n",
        response.status().as_u16()
    );
    for (key, value) in response.headers() {
        if !matches!(
            key.as_str(),
            "content-length" | "transfer-encoding" | "connection"
        ) {
            head.push_str(&format!("{key}: {}\r\n", value.to_str()?));
        }
    }
    let bytes = response.bytes().await?;
    head.push_str(&format!("Content-Length: {}\r\n\r\n", bytes.len()));
    socket.write_all(head.as_bytes()).await?;
    socket.write_all(&bytes).await?;
    Ok(())
}

#[tokio::test]
async fn oversized_files_are_rejected_for_all_body_encodings() {
    let college = College::start(false).await;
    for body in [Body::Length, Body::Chunked, Body::UntilClose, Body::Gzip] {
        let server = Server::start(&college, body, FILE_LIMIT + 1).await;
        let client = server.login().await;
        let error = tokio::time::timeout(Duration::from_secs(10), client.download(10))
            .await
            .expect("size limit must abort without waiting for more data")
            .unwrap_err();
        assert!(
            error.to_string().contains("file exceeds download limit"),
            "{error:#}"
        );
    }
}

#[tokio::test]
async fn exact_limit_is_accepted_and_temporary_file_is_removed() {
    let college = College::start(false).await;
    let server = Server::start(&college, Body::Chunked, FILE_LIMIT).await;
    let client = server.login().await;
    let (file, _) = client.download(10).await.unwrap();
    assert_eq!(
        tokio::fs::metadata(&file).await.unwrap().len(),
        FILE_LIMIT as u64
    );
    let path = file.to_path_buf();
    drop(file);
    assert!(!path.exists());
}

#[tokio::test]
async fn stalled_file_body_obeys_download_timeout() {
    let college = College::start(false).await;
    let server = Server::start(&college, Body::Slow, 0).await;
    let client = server.login().await;
    let download = tokio::spawn(async move { client.download(10).await });
    tokio::time::timeout(Duration::from_secs(5), server.body_started.notified())
        .await
        .unwrap();
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(61)).await;
    let error = download.await.unwrap().unwrap_err();
    assert!(
        error
            .downcast_ref::<reqwest::Error>()
            .is_some_and(|e| e.is_timeout()),
        "{error:#}"
    );
}
