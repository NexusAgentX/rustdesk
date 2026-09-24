use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::{body::Incoming, server::conn::http1, service::service_fn, Request, Response};
use hyper_util::rt::TokioIo;
use std::{convert::Infallible, error::Error};

pub async fn serve_once(listener: tokio::net::TcpListener) -> Result<(), Box<dyn Error + Send + Sync>> {
    let (stream, _) = listener.accept().await?;
    http1::Builder::new().serve_connection(TokioIo::new(stream), service_fn(|request: Request<Incoming>| async move {
        let result = Limited::new(request.into_body(), 4096).collect().await;
        let body = match result {
            Ok(bytes) => match serde_json::from_slice::<serde_json::Value>(&bytes.to_bytes()) {
                Ok(value) => serde_json::to_vec(&value).unwrap_or_default(),
                Err(_) => b"invalid JSON".to_vec(),
            },
            Err(_) => b"body too large".to_vec(),
        };
        Ok::<_, Infallible>(Response::new(Full::new(Bytes::from(body))))
    })).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    #[tokio::test]
    async fn bounded_json_http_roundtrip() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(serve_once(listener));
        let mut client = tokio::net::TcpStream::connect(address).await.unwrap();
        client.write_all(b"POST / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: 11\r\n\r\n{\"ok\":true}").await.unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        assert!(String::from_utf8(response).unwrap().contains("{\"ok\":true}"));
        server.await.unwrap().unwrap();
    }
}
