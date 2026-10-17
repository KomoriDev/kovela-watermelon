//! 不阻塞插件实例的 HTTP 请求（wasi:http 0.2，与 waki 用同一份绑定，不新增导入）。
//!
//! waki 的 `send()` / `body()` 在等待网络时调用 `pollable.block()`：这是同步的
//! 宿主调用，期间整个插件实例都被占住，宿主送不进任何点击事件——购买验证一慢，
//! 整个换肤页面就像卡死。与 VelaPass 的 Kovela 插件共用同一份实现。这里改成轮询 `ready()`，
//! 没就绪就 `yield` 把执行权交还宿主，并给整次请求一个总截止时间（waki 只能设
//! 连接超时，服务端连上后不回包会一直等下去）。

use std::time::{Duration, Instant};
use waki::bindings::wasi::{
    http::{
        outgoing_handler,
        types::{Fields, Method, OutgoingBody, OutgoingRequest, RequestOptions, Scheme},
    },
    io::{poll::Pollable, streams::StreamError},
};

/// 响应体读取上限：业务响应都远小于它（解析时还会再按 16 KiB 拒绝）。
const MAX_BODY: usize = 64 * 1024;

#[derive(Debug, PartialEq)]
pub(crate) enum NetError {
    /// 没连上，或宿主拒绝了请求。
    Connect,
    /// 总截止时间到了还没收完。
    Timeout,
    /// 连上了但响应没读完整。
    Body,
}

pub(crate) struct Reply {
    pub status: u16,
    pub body: Vec<u8>,
}

async fn ready(pollable: &Pollable, deadline: Instant) -> Result<(), NetError> {
    while !pollable.ready() {
        if Instant::now() >= deadline {
            return Err(NetError::Timeout);
        }
        wit_bindgen::yield_async().await;
    }
    Ok(())
}

/// 把 `https://host[:port]` 形式的源站拆成 (scheme, authority)。
fn split_origin(origin: &str) -> Option<(Scheme, &str)> {
    let (scheme, authority) = if let Some(rest) = origin.strip_prefix("https://") {
        (Scheme::Https, rest)
    } else if let Some(rest) = origin.strip_prefix("http://") {
        (Scheme::Http, rest)
    } else {
        return None;
    };
    let authority = authority.trim_end_matches('/');
    (!authority.is_empty() && !authority.contains('/')).then_some((scheme, authority))
}

pub(crate) async fn send(
    method: Method,
    origin: &str,
    path: &str,
    body: Option<&[u8]>,
    connect_timeout: Duration,
    total_timeout: Duration,
) -> Result<Reply, NetError> {
    let deadline = Instant::now() + total_timeout;
    let (scheme, authority) = split_origin(origin).ok_or(NetError::Connect)?;
    let mut headers = Vec::new();
    if body.is_some() {
        headers.push(("Content-Type".to_string(), b"application/json".to_vec()));
    }
    let headers = Fields::from_list(&headers).map_err(|_| NetError::Connect)?;
    let request = OutgoingRequest::new(headers);
    request
        .set_method(&method)
        .map_err(|()| NetError::Connect)?;
    request
        .set_scheme(Some(&scheme))
        .map_err(|()| NetError::Connect)?;
    request
        .set_authority(Some(authority))
        .map_err(|()| NetError::Connect)?;
    request
        .set_path_with_query(Some(path))
        .map_err(|()| NetError::Connect)?;
    let outgoing = request.body().map_err(|()| NetError::Connect)?;

    let options = RequestOptions::new();
    let nanos = |value: Duration| value.as_nanos().min(u64::MAX as u128) as u64;
    // 宿主不支持某个超时项时会返回错误：忽略即可，总截止时间兜底。
    let _ = options.set_connect_timeout(Some(nanos(connect_timeout)));
    let _ = options.set_first_byte_timeout(Some(nanos(total_timeout)));
    let _ = options.set_between_bytes_timeout(Some(nanos(total_timeout)));
    let pending =
        outgoing_handler::handle(request, Some(options)).map_err(|_| NetError::Connect)?;

    if let Some(mut data) = body {
        let stream = outgoing.write().map_err(|()| NetError::Connect)?;
        {
            let pollable = stream.subscribe();
            while !data.is_empty() {
                ready(&pollable, deadline).await?;
                let permit = stream.check_write().map_err(|_| NetError::Connect)? as usize;
                if permit == 0 {
                    continue;
                }
                let (chunk, rest) = data.split_at(permit.min(data.len()));
                stream.write(chunk).map_err(|_| NetError::Connect)?;
                data = rest;
            }
            stream.flush().map_err(|_| NetError::Connect)?;
            ready(&pollable, deadline).await?;
            stream.check_write().map_err(|_| NetError::Connect)?;
        }
        drop(stream);
    }
    OutgoingBody::finish(outgoing, None).map_err(|_| NetError::Connect)?;

    {
        let pollable = pending.subscribe();
        ready(&pollable, deadline).await?;
    }
    let response = match pending.get() {
        Some(Ok(Ok(response))) => response,
        _ => return Err(NetError::Connect),
    };
    drop(pending);
    let status = response.status();
    let incoming = response.consume().map_err(|()| NetError::Body)?;
    let mut bytes = Vec::new();
    {
        let stream = incoming.stream().map_err(|()| NetError::Body)?;
        {
            let pollable = stream.subscribe();
            loop {
                match stream.read(16 * 1024) {
                    Ok(chunk) if chunk.is_empty() => ready(&pollable, deadline).await?,
                    Ok(chunk) => {
                        bytes.extend_from_slice(&chunk);
                        if bytes.len() > MAX_BODY {
                            break;
                        }
                    }
                    Err(StreamError::Closed) => break,
                    Err(_) => return Err(NetError::Body),
                }
            }
        }
        drop(stream);
    }
    drop(incoming);
    drop(response);
    Ok(Reply {
        status,
        body: bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_only_bare_origins() {
        assert!(matches!(
            split_origin("https://kovela.komoridevs.icu"),
            Some((Scheme::Https, "kovela.komoridevs.icu"))
        ));
        assert!(matches!(
            split_origin("http://127.0.0.1:8787/"),
            Some((Scheme::Http, "127.0.0.1:8787"))
        ));
        assert!(split_origin("https://host/path").is_none());
        assert!(split_origin("ftp://host").is_none());
        assert!(split_origin("https://").is_none());
    }
}
