use std::collections::VecDeque;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio_rustls::rustls::ClientConfig;
use tokio_rustls::TlsConnector;

use super::pool::PoolConnection;

pub struct StratumStream {
    reader: BufReader<Box<dyn tokio::io::AsyncRead + Unpin + Send>>,
    writer: Box<dyn tokio::io::AsyncWrite + Unpin + Send>,
    /// Pending parsed JSON messages from a previously-read line. NiceHash
    /// occasionally packs multiple JSON-RPC objects into a single newline-
    /// terminated frame (e.g. mining.set_difficulty + mining.notify together);
    /// we split those out at read time so callers always see one message
    /// per `read_line()` call.
    pending: VecDeque<String>,
}

impl StratumStream {
    pub async fn connect(
        pool: &PoolConnection,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let addr = pool.address();
        log::info!("connecting to \x1b[1m{}\x1b[0m...", addr);

        if pool.tls {
            let config = ClientConfig::builder()
                .with_root_certificates(Self::root_store())
                .with_no_client_auth();

            let connector = TlsConnector::from(Arc::new(config));
            let stream = TcpStream::connect(&addr).await?;
            stream.set_nodelay(true)?;

            let server_name = pool
                .host
                .clone()
                .try_into()
                .map_err(|_| format!("Invalid server name: {}", pool.host))?;
            let tls_stream = connector.connect(server_name, stream).await?;

            let (read, write) = tokio::io::split(tls_stream);
            Ok(StratumStream {
                reader: BufReader::new(Box::new(read)),
                writer: Box::new(write),
                pending: VecDeque::new(),
            })
        } else {
            let stream = TcpStream::connect(&addr).await?;
            stream.set_nodelay(true)?;
            let (read, write) = tokio::io::split(stream);
            Ok(StratumStream {
                reader: BufReader::new(Box::new(read)),
                writer: Box::new(write),
                pending: VecDeque::new(),
            })
        }
    }

    /// Read one JSON-RPC message from the stream.
    ///
    /// A single TCP frame may contain multiple newline-terminated JSON
    /// objects, or even multiple JSONs without intervening newlines (some
    /// pools concatenate set_difficulty + notify). We use serde_json's
    /// streaming parser to peel off one object at a time and queue the rest.
    pub async fn read_line(&mut self) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        if let Some(msg) = self.pending.pop_front() {
            return Ok(msg);
        }
        let mut buf = String::new();
        let n = self.reader.read_line(&mut buf).await?;
        if n == 0 {
            return Err("Connection closed".into());
        }
        let trimmed = buf.trim();
        if trimmed.is_empty() {
            return Box::pin(self.read_line()).await;
        }
        let stream = serde_json::Deserializer::from_str(trimmed)
            .into_iter::<serde_json::Value>();
        let mut first: Option<String> = None;
        for v in stream {
            let v = v?;
            let s = serde_json::to_string(&v)?;
            if first.is_none() {
                first = Some(s);
            } else {
                self.pending.push_back(s);
            }
        }
        first.ok_or_else(|| "no JSON messages in line".into())
    }

    pub async fn write_line(
        &mut self,
        data: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let data = format!("{}\n", data);
        self.writer.write_all(data.as_bytes()).await?;
        self.writer.flush().await?;
        Ok(())
    }

    fn root_store() -> rustls::RootCertStore {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        roots
    }
}
