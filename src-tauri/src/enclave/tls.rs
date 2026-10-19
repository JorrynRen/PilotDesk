//! 主机服务 TLS（M9）：自签证书（rustls + rcgen）+ 证书指纹 pin。
//!
//! 设计取舍：
//! - **自签证书**：局域网没有公信 CA，主机首次启动用 `rcgen` 生成自签证书并持久化；
//! - **指纹 pin**：客户端在 TLS 握手阶段校验对端证书 sha256（首次 TOFU 记录，之后必须一致）；
//! - **协议零变化**：TLS 只替换 accept/connect 层，帧结构（hello / req / res）不变。
//!
//! 为什么用 rustls 而不是 native-tls：Windows SChannel 的 `Identity::from_pkcs8`
//! 走 `rsa_full` provider，不接受 rcgen 默认生成的 ECDSA 密钥；rustls 对 ECDSA 原生支持，
//! 且可在握手中直接做指纹校验（无需先信任再事后比对）。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use sha2::{Digest, Sha256};
use tokio::net::TcpStream;
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

const HOST_CERT_NAME: &str = "host-cert.der";
const HOST_KEY_NAME: &str = "host-key.der";

/// 主机 TLS 运行件：accept 器 + 本机证书指纹。
pub struct HostTls {
    pub acceptor: TlsAcceptor,
    pub fingerprint: String,
}

fn tls_dir() -> PathBuf {
    crate::utils::paths::app_root_dir()
        .join("enclave")
        .join("tls")
}

/// 加载（首次则生成）本机自签证书。
pub fn ensure_host_tls() -> Result<HostTls, String> {
    ensure_host_tls_in(&tls_dir())
}

/// 加载（首次则生成）指定目录下的自签证书（测试可注入临时目录）。
pub fn ensure_host_tls_in(dir: &Path) -> Result<HostTls, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("创建 TLS 目录失败：{}", e))?;
    let cert_path = dir.join(HOST_CERT_NAME);
    let key_path = dir.join(HOST_KEY_NAME);
    if !cert_path.exists() || !key_path.exists() {
        let certified = rcgen::generate_simple_self_signed(vec![
            "pilotdesk-enclave".to_string(),
            "localhost".to_string(),
        ])
        .map_err(|e| format!("生成主机证书失败：{}", e))?;
        std::fs::write(&cert_path, certified.cert.der().as_ref())
            .map_err(|e| format!("写入主机证书失败：{}", e))?;
        std::fs::write(&key_path, certified.key_pair.serialize_der())
            .map_err(|e| format!("写入主机私钥失败：{}", e))?;
        log::info!("[enclave] 已生成主机自签证书：{}", cert_path.display());
    }

    let cert_der =
        std::fs::read(&cert_path).map_err(|e| format!("读取主机证书失败：{}", e))?;
    let key_der = std::fs::read(&key_path).map_err(|e| format!("读取主机私钥失败：{}", e))?;
    let fingerprint = hex_sha256(&cert_der);

    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(cert_der)],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_der)),
        )
        .map_err(|e| format!("创建 TLS 服务端配置失败：{}", e))?;
    Ok(HostTls {
        acceptor: TlsAcceptor::from(Arc::new(config)),
        fingerprint,
    })
}

/// 本机证书指纹（十六进制 sha256；不存在则生成）。
pub fn host_fingerprint() -> Result<String, String> {
    ensure_host_tls().map(|t| t.fingerprint)
}

fn hex_sha256(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{:02x}", b)).collect()
}

/// 只按证书指纹校验的 TLS 校验器（自签证书不做系统链校验）。
#[derive(Debug)]
struct FingerprintVerifier {
    expected: Option<String>,
    observed: Arc<Mutex<Option<String>>>,
}

impl ServerCertVerifier for FingerprintVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let fingerprint = hex_sha256(end_entity.as_ref());
        if let Some(expected) = self
            .expected
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            if fingerprint != expected {
                return Err(rustls::Error::General(format!(
                    "证书指纹不匹配（期望 {}，实际 {}）",
                    short(expected),
                    short(&fingerprint)
                )));
            }
        }
        *self.observed.lock().unwrap_or_else(|e| e.into_inner()) = Some(fingerprint);
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// 建立 `wss://` 连接，并在握手阶段校验主机证书指纹。
///
/// `expected_fingerprint` 为空表示首次连接（TOFU）：返回实际指纹，由调用方持久化。
pub async fn connect_pinned(
    url: &str,
    expected_fingerprint: Option<&str>,
    timeout: Duration,
) -> Result<(WebSocketStream<MaybeTlsStream<TcpStream>>, String), String> {
    let observed = Arc::new(Mutex::new(None));
    let config = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(FingerprintVerifier {
            expected: expected_fingerprint.map(str::to_string),
            observed: observed.clone(),
        }))
        .with_no_client_auth();
    let connector = tokio_tungstenite::Connector::Rustls(Arc::new(config));

    let (ws, _) = tokio::time::timeout(
        timeout,
        tokio_tungstenite::connect_async_tls_with_config(url, None, false, Some(connector)),
    )
    .await
    .map_err(|_| "连接主机超时".to_string())?
    .map_err(|e| {
        let msg = e.to_string();
        if msg.contains("证书指纹不匹配") {
            format!("主机证书指纹不匹配，已拒绝连接：{}", msg)
        } else {
            format!("TLS 连接主机失败：{}", msg)
        }
    })?;

    let fingerprint = observed
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .ok_or_else(|| "未能读取主机证书指纹".to_string())?;
    Ok((ws, fingerprint))
}

fn short(fp: &str) -> &str {
    fp.get(..16).unwrap_or(fp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::{SinkExt, StreamExt};
    use tokio::net::TcpListener;
    use tokio_tungstenite::tungstenite::protocol::Message;

    /// 自签 TLS + 指纹 pin：首次 TOFU 成功，指纹不匹配被拒。
    #[tokio::test]
    async fn tls_roundtrip_and_pin_mismatch() {
        let dir = std::env::temp_dir().join(format!(
            "pilotdesk-enclave-tls-test-{}",
            crate::utils::new_id()
        ));
        let host = ensure_host_tls_in(&dir).unwrap();

        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let acceptor = host.acceptor.clone();
        tokio::spawn(async move {
            // 接受多次连接：第一次成功，第二次用于验证指纹不匹配会被客户端拒绝
            while let Ok((stream, _)) = listener.accept().await {
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    if let Ok(tls) = acceptor.accept(stream).await {
                        if let Ok(mut ws) = tokio_tungstenite::accept_async(tls).await {
                            let _ = ws.send(Message::Text("pong".to_string())).await;
                        }
                    }
                });
            }
        });

        let url = format!("wss://127.0.0.1:{}", port);
        let (mut ws, fp) = connect_pinned(&url, None, Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(fp, host.fingerprint);
        let msg = ws.next().await.unwrap().unwrap();
        assert_eq!(msg.to_text().unwrap(), "pong");
        drop(ws);

        // 指纹不匹配：必须拒绝
        let err = connect_pinned(&url, Some(&"00".repeat(32)), Duration::from_secs(5))
            .await
            .err()
            .expect("指纹不匹配应失败");
        assert!(err.contains("指纹不匹配"), "unexpected error: {}", err);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
