//! 节点身份（libp2p Ed25519 密钥对）生成与持久化

use libp2p::identity::Keypair;
use std::path::PathBuf;

/// 身份文件路径：~/.cc-switch-remote/share/identity.key
pub fn identity_path() -> PathBuf {
    super::config::share_data_dir().join("identity.key")
}

/// 加载或创建节点身份密钥对
///
/// 文件不存在时生成新的 Ed25519 密钥对并以 0600 权限持久化；
/// 重启后 PeerId 保持不变（出借方黑名单/限额均按 PeerId 归因）。
pub fn load_or_create_identity() -> Result<Keypair, String> {
    let path = identity_path();
    if path.exists() {
        let bytes = std::fs::read(&path).map_err(|e| format!("读取节点身份失败: {e}"))?;
        Keypair::from_protobuf_encoding(&bytes).map_err(|e| format!("解析节点身份失败: {e}"))
    } else {
        let keypair = Keypair::generate_ed25519();
        let bytes = keypair
            .to_protobuf_encoding()
            .map_err(|e| format!("编码节点身份失败: {e}"))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            let mut opts = std::fs::OpenOptions::new();
            opts.write(true).create_new(true).mode(0o600);
            let mut file = opts
                .open(&path)
                .map_err(|e| format!("写入节点身份失败: {e}"))?;
            use std::io::Write;
            file.write_all(&bytes)
                .map_err(|e| format!("写入节点身份失败: {e}"))?;
        }
        #[cfg(not(unix))]
        {
            std::fs::write(&path, &bytes).map_err(|e| format!("写入节点身份失败: {e}"))?;
        }

        Ok(keypair)
    }
}

/// 从密钥对导出 PeerId 字符串
pub fn peer_id_string(keypair: &Keypair) -> String {
    libp2p::PeerId::from(keypair.public()).to_base58()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_roundtrip() {
        let kp = Keypair::generate_ed25519();
        let bytes = kp.to_protobuf_encoding().unwrap();
        let kp2 = Keypair::from_protobuf_encoding(&bytes).unwrap();
        assert_eq!(peer_id_string(&kp), peer_id_string(&kp2));
    }
}
