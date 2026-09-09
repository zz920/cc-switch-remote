//! 节点身份密钥（Ed25519）的加载与首次生成。
//!
//! 密钥以 libp2p protobuf 编码持久化到磁盘文件（默认 `relay.key`，权限 0600），
//! 保证服务器重启后 PeerId 不变，客户端无需更新 bootstrap 地址。

use std::{
    fs::{self, OpenOptions},
    io::{ErrorKind, Write},
    path::Path,
};

use libp2p::identity::Keypair;
use tracing::info;

/// 从 `path` 加载密钥对；文件不存在时生成新密钥并写入（0600 权限）
pub fn load_or_generate(path: &Path) -> std::io::Result<Keypair> {
    match fs::read(path) {
        Ok(bytes) => {
            let keypair = Keypair::from_protobuf_encoding(&bytes).map_err(|err| {
                std::io::Error::new(
                    ErrorKind::InvalidData,
                    format!("{}: 密钥文件内容损坏或格式不正确: {err}", path.display()),
                )
            })?;
            info!(path = %path.display(), "已加载现有节点密钥");
            Ok(keypair)
        }
        Err(err) if err.kind() == ErrorKind::NotFound => {
            let keypair = Keypair::generate_ed25519();
            let encoded = keypair.to_protobuf_encoding().map_err(|err| {
                std::io::Error::new(
                    ErrorKind::InvalidData,
                    format!("密钥 protobuf 编码失败: {err}"),
                )
            })?;
            write_private_file(path, &encoded)?;
            info!(path = %path.display(), "首次启动，已生成新的 Ed25519 节点密钥");
            Ok(keypair)
        }
        Err(err) => Err(err),
    }
}

/// 以仅属主可读写（0600）的权限创建并写入文件。
///
/// 使用 `create_new` 避免并发/链接攻击下覆盖已有文件；
/// 先以 0600 模式创建，写完后再显式设置一次权限兜底（如 umask 干扰）。
fn write_private_file(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    let mut file = options.open(path)?;
    file.write_all(contents)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }

    Ok(())
}
