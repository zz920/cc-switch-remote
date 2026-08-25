//! Share key 安全存储：OS keychain 优先，降级为 0600 文件
//!
//! key 永不进入数据库与日志；降级存储时前端会提示风险。

use super::config::share_data_dir;
use serde::Serialize;

const KEYRING_SERVICE: &str = "tokentap";

/// key 的实际存储位置（用于 UI 风险提示）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum KeyStorage {
    /// OS 钥匙串（macOS Keychain / Windows 凭据管理器 / Linux Secret Service）
    Keyring,
    /// 降级：~/.tokentap/share/secret-<hash>（0600）
    File,
}

fn keyring_entry(hash: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(KEYRING_SERVICE, &format!("share-key-{hash}"))
        .map_err(|e| format!("初始化 keyring 失败: {e}"))
}

fn fallback_path(hash: &str) -> std::path::PathBuf {
    share_data_dir().join(format!("secret-{hash}"))
}

fn write_fallback_file(hash: &str, key: &str) -> Result<(), String> {
    let path = fallback_path(hash);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true).mode(0o600);
        let mut file = opts
            .open(&path)
            .map_err(|e| format!("写入密钥文件失败: {e}"))?;
        use std::io::Write;
        file.write_all(key.as_bytes())
            .map_err(|e| format!("写入密钥文件失败: {e}"))?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(&path, key).map_err(|e| format!("写入密钥文件失败: {e}"))?;
    }
    Ok(())
}

/// 保存 share key：优先 keychain，失败降级文件
pub fn save_share_key(hash: &str, key: &str) -> Result<KeyStorage, String> {
    match keyring_entry(hash).and_then(|entry| {
        entry
            .set_password(key)
            .map_err(|e| format!("keyring 写入失败: {e}"))
    }) {
        Ok(()) => {
            // 清理可能存在的降级文件
            let _ = std::fs::remove_file(fallback_path(hash));
            Ok(KeyStorage::Keyring)
        }
        Err(e) => {
            log::warn!("[Share] keyring 不可用（{e}），降级为文件存储");
            write_fallback_file(hash, key)?;
            Ok(KeyStorage::File)
        }
    }
}

/// 读取 share key（先 keychain 后文件）
pub fn load_share_key(hash: &str) -> Result<Option<String>, String> {
    if let Ok(entry) = keyring_entry(hash) {
        match entry.get_password() {
            Ok(key) => return Ok(Some(key)),
            Err(keyring::Error::NoEntry) => {}
            Err(e) => {
                log::debug!("[Share] keyring 读取失败（尝试文件降级）: {e}");
            }
        }
    }
    let path = fallback_path(hash);
    if path.exists() {
        let key = std::fs::read_to_string(&path).map_err(|e| format!("读取密钥文件失败: {e}"))?;
        return Ok(Some(key.trim().to_string()));
    }
    Ok(None)
}

/// 删除 share key（退出网络 / 解散时调用）
pub fn delete_share_key(hash: &str) -> Result<(), String> {
    if let Ok(entry) = keyring_entry(hash) {
        let _ = entry.delete_credential();
    }
    let _ = std::fs::remove_file(fallback_path(hash));
    Ok(())
}

/// 当前 key 的存储位置（供 UI 风险提示）
pub fn current_storage(hash: &str) -> KeyStorage {
    if let Ok(entry) = keyring_entry(hash) {
        if entry.get_password().is_ok() {
            return KeyStorage::Keyring;
        }
    }
    if fallback_path(hash).exists() {
        return KeyStorage::File;
    }
    // 默认按 keychain 报告（尚未存储时）
    KeyStorage::Keyring
}
