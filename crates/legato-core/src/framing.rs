//! 스트림 위 메시지 경계: `u32` 리틀엔디언 길이 + postcard 직렬화 본문.

use anyhow::{Context, Result, bail};
use serde::{Serialize, de::DeserializeOwned};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// 메시지 하나의 최대 크기. 영상 키프레임까지 들어가게 넉넉히 잡는다.
pub const MAX_MESSAGE_LEN: usize = 32 * 1024 * 1024;

/// 메시지 하나를 직렬화해 보낸다.
pub async fn write_msg<W, T>(w: &mut W, msg: &T) -> Result<()>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let body = postcard::to_stdvec(msg).context("메시지 직렬화 실패")?;
    if body.len() > MAX_MESSAGE_LEN {
        bail!("메시지가 너무 큼: {} 바이트", body.len());
    }
    let mut buf = Vec::with_capacity(4 + body.len());
    buf.extend_from_slice(&(body.len() as u32).to_le_bytes());
    buf.extend_from_slice(&body);
    w.write_all(&buf).await.context("스트림 쓰기 실패")?;
    Ok(())
}

/// 메시지 하나를 읽는다. 스트림이 깔끔하게 끝났으면 `None`.
pub async fn read_msg<R, T>(r: &mut R) -> Result<Option<T>>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    let mut len = [0u8; 4];
    match r.read_exact(&mut len).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e).context("스트림 읽기 실패"),
    }
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_MESSAGE_LEN {
        bail!("메시지 길이 비정상: {len} 바이트");
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).await.context("메시지 본문 읽기 실패")?;
    let msg = postcard::from_bytes(&body).context("메시지 역직렬화 실패")?;
    Ok(Some(msg))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{ClientMsg, InputEvent};

    #[tokio::test]
    async fn 왕복_직렬화() {
        let (mut a, mut b) = tokio::io::duplex(1024);
        let msg = ClientMsg::Input(InputEvent::Key { code: "KeyA".into(), down: true });
        write_msg(&mut a, &msg).await.unwrap();
        drop(a);
        let got: ClientMsg = read_msg(&mut b).await.unwrap().unwrap();
        assert!(matches!(got, ClientMsg::Input(InputEvent::Key { ref code, down: true }) if code == "KeyA"));
        let end: Option<ClientMsg> = read_msg(&mut b).await.unwrap();
        assert!(end.is_none());
    }
}
