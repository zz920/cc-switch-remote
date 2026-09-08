//! 组网（TokenTap Share）数据访问层
//!
//! - `share_network`：单行网络配置（share id、角色、路由偏好、白名单、限额）
//! - `share_blocked_peers`：被拉黑的节点
//! - 出借用量归因复用 `proxy_request_logs`：出借侧使用按 peer 合成的
//!   provider_id（`sharelend:<peer>:<provider_id>`），此处按前缀聚合。
//! - 消费用量同样复用 `proxy_request_logs`：消费侧远端路由使用
//!   provider_id（`share:<peer>`），此处按前缀聚合。

use super::super::lock_conn;
use crate::database::Database;
use crate::error::AppError;
use rusqlite::params;
use serde::{Deserialize, Serialize};

/// 组网网络配置（单行）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareNetworkRow {
    /// 可读 share id（8 位）
    pub share_id: String,
    /// rendezvous 命名空间后缀：sha256(share_id) 前 16 位 hex
    pub share_id_hash: String,
    /// 角色：creator / member
    pub role: String,
    /// 消费侧路由偏好：local_first / network_first / network_only
    pub route_preference: String,
    /// relay 地址覆盖（None = 官方默认）
    pub relay_addr: Option<String>,
    /// 出借白名单：JSON 数组，元素形如 "claude:<provider_id>"
    pub shared_provider_ids: Vec<String>,
    /// 限额周期：daily / monthly
    pub quota_scope: String,
    /// 限额 token 上限（0 = 不限）
    pub quota_max_tokens: i64,
    /// 是否按 peer 分别限额
    pub quota_per_peer: bool,
    /// 节点显示名（能力通告用）
    pub node_name: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// 被拉黑的节点
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareBlockedPeerRow {
    pub peer_id: String,
    pub reason: Option<String>,
    pub created_at: i64,
}

/// 出借侧合成 provider_id 前缀（用量归因，见 proxy_request_logs.provider_id）
pub const SHARE_LEND_PROVIDER_PREFIX: &str = "sharelend";

/// 消费侧合成 provider_id 前缀（见 share::route_hook）
pub const SHARE_REMOTE_PROVIDER_PREFIX: &str = "share";

/// 组装出借侧合成 provider_id
pub fn lend_provider_id(peer_id: &str, provider_id: &str) -> String {
    format!("{SHARE_LEND_PROVIDER_PREFIX}:{peer_id}:{provider_id}")
}

impl Database {
    /// 保存（新建或覆盖）组网网络配置
    pub fn save_share_network(&self, row: &ShareNetworkRow) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        let whitelist =
            serde_json::to_string(&row.shared_provider_ids).unwrap_or_else(|_| "[]".to_string());
        conn.execute(
            "INSERT INTO share_network (
                id, share_id, share_id_hash, role, route_preference, relay_addr,
                shared_provider_ids, quota_scope, quota_max_tokens, quota_per_peer,
                node_name, created_at, updated_at
             ) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             ON CONFLICT(id) DO UPDATE SET
                share_id = excluded.share_id,
                share_id_hash = excluded.share_id_hash,
                role = excluded.role,
                route_preference = excluded.route_preference,
                relay_addr = excluded.relay_addr,
                shared_provider_ids = excluded.shared_provider_ids,
                quota_scope = excluded.quota_scope,
                quota_max_tokens = excluded.quota_max_tokens,
                quota_per_peer = excluded.quota_per_peer,
                node_name = excluded.node_name,
                updated_at = excluded.updated_at",
            params![
                row.share_id,
                row.share_id_hash,
                row.role,
                row.route_preference,
                row.relay_addr,
                whitelist,
                row.quota_scope,
                row.quota_max_tokens,
                row.quota_per_peer as i64,
                row.node_name,
                row.created_at,
                row.updated_at,
            ],
        )
        .map_err(|e| AppError::Database(format!("保存组网配置失败: {e}")))?;
        Ok(())
    }

    /// 读取组网网络配置（未加入任何网络时返回 None）
    pub fn get_share_network(&self) -> Result<Option<ShareNetworkRow>, AppError> {
        let conn = lock_conn!(self.conn);
        let mut stmt = conn
            .prepare(
                "SELECT share_id, share_id_hash, role, route_preference, relay_addr,
                        shared_provider_ids, quota_scope, quota_max_tokens, quota_per_peer,
                        node_name, created_at, updated_at
                 FROM share_network WHERE id = 1",
            )
            .map_err(|e| AppError::Database(e.to_string()))?;
        let row = stmt
            .query_row([], |r| {
                let whitelist_json: String = r.get(5)?;
                Ok(ShareNetworkRow {
                    share_id: r.get(0)?,
                    share_id_hash: r.get(1)?,
                    role: r.get(2)?,
                    route_preference: r.get(3)?,
                    relay_addr: r.get(4)?,
                    shared_provider_ids: serde_json::from_str(&whitelist_json).unwrap_or_default(),
                    quota_scope: r.get(6)?,
                    quota_max_tokens: r.get(7)?,
                    quota_per_peer: r.get::<_, i64>(8)? != 0,
                    node_name: r.get(9)?,
                    created_at: r.get(10)?,
                    updated_at: r.get(11)?,
                })
            })
            .ok();
        Ok(row)
    }

    /// 删除组网网络配置（退出网络）
    pub fn delete_share_network(&self) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        conn.execute("DELETE FROM share_network WHERE id = 1", [])
            .map_err(|e| AppError::Database(format!("删除组网配置失败: {e}")))?;
        Ok(())
    }

    /// 拉黑节点
    pub fn block_share_peer(&self, peer_id: &str, reason: Option<&str>) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        conn.execute(
            "INSERT INTO share_blocked_peers (peer_id, reason, created_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(peer_id) DO UPDATE SET reason = excluded.reason",
            params![peer_id, reason, chrono::Utc::now().timestamp()],
        )
        .map_err(|e| AppError::Database(format!("拉黑节点失败: {e}")))?;
        Ok(())
    }

    /// 解除拉黑
    pub fn unblock_share_peer(&self, peer_id: &str) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        conn.execute(
            "DELETE FROM share_blocked_peers WHERE peer_id = ?1",
            params![peer_id],
        )
        .map_err(|e| AppError::Database(format!("解除拉黑失败: {e}")))?;
        Ok(())
    }

    /// 查询节点是否被拉黑
    pub fn is_share_peer_blocked(&self, peer_id: &str) -> Result<bool, AppError> {
        let conn = lock_conn!(self.conn);
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM share_blocked_peers WHERE peer_id = ?1",
                params![peer_id],
                |r| r.get(0),
            )
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(count > 0)
    }

    /// 列出全部黑名单节点
    pub fn list_share_blocked_peers(&self) -> Result<Vec<ShareBlockedPeerRow>, AppError> {
        let conn = lock_conn!(self.conn);
        let mut stmt = conn
            .prepare(
                "SELECT peer_id, reason, created_at FROM share_blocked_peers ORDER BY created_at DESC",
            )
            .map_err(|e| AppError::Database(e.to_string()))?;
        let rows = stmt
            .query_map([], |r| {
                Ok(ShareBlockedPeerRow {
                    peer_id: r.get(0)?,
                    reason: r.get(1)?,
                    created_at: r.get(2)?,
                })
            })
            .map_err(|e| AppError::Database(e.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(rows)
    }

    /// 统计某 peer 在指定时间点后已消耗的 token 总量
    ///
    /// 归因口径：`proxy_request_logs.provider_id` 形如 `sharelend:<peer>:<id>`，
    /// token 总量 = input + output + cache_read + cache_creation。
    pub fn share_peer_tokens_used(&self, peer_id: &str, since_epoch: i64) -> Result<i64, AppError> {
        let conn = lock_conn!(self.conn);
        let like = format!("{SHARE_LEND_PROVIDER_PREFIX}:{peer_id}:%");
        let total: i64 = conn
            .query_row(
                "SELECT COALESCE(SUM(
                    input_tokens + output_tokens + cache_read_tokens + cache_creation_tokens
                 ), 0)
                 FROM proxy_request_logs
                 WHERE provider_id LIKE ?1 AND created_at >= ?2 AND status_code < 400",
                params![like, since_epoch],
                |r| r.get(0),
            )
            .map_err(|e| AppError::Database(format!("统计组网用量失败: {e}")))?;
        Ok(total)
    }

    /// 统计全部 peer 的组网用量（用于节点列表展示）
    ///
    /// 返回 (peer_id, tokens) 列表。peer_id 从合成 provider_id 中提取。
    pub fn share_all_peers_tokens_used(
        &self,
        since_epoch: i64,
    ) -> Result<Vec<(String, i64)>, AppError> {
        let conn = lock_conn!(self.conn);
        let prefix = format!("{SHARE_LEND_PROVIDER_PREFIX}:%");
        let mut stmt = conn
            .prepare(
                "SELECT provider_id, COALESCE(SUM(
                    input_tokens + output_tokens + cache_read_tokens + cache_creation_tokens
                 ), 0) AS total
                 FROM proxy_request_logs
                 WHERE provider_id LIKE ?1 AND created_at >= ?2 AND status_code < 400
                 GROUP BY provider_id",
            )
            .map_err(|e| AppError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![prefix, since_epoch], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
            })
            .map_err(|e| AppError::Database(e.to_string()))?;
        let mut result: std::collections::HashMap<String, i64> = Default::default();
        for row in rows {
            let (provider_id, tokens) = row.map_err(|e| AppError::Database(e.to_string()))?;
            // provider_id 形如 sharelend:<peer>:<id>，取中段为 peer
            if let Some(peer) = provider_id.split(':').nth(1) {
                *result.entry(peer.to_string()).or_insert(0) += tokens;
            }
        }
        Ok(result.into_iter().collect())
    }

    /// 统计本机通过共享网络消费的 token 总量。
    ///
    /// 消费侧合成 provider_id 形如 `share:<peer>`；成功请求的 token 总量
    /// 与出借侧保持同一口径：input + output + cache_read + cache_creation。
    /// 消费侧按完整 provider_id（`share:<peer>:<provider_id>`）聚合的共享消费
    /// token 数。供路由卡逐行展示「该节点的此供应商已用了多少」。
    pub fn share_remote_provider_tokens_used(
        &self,
        since_epoch: i64,
    ) -> Result<Vec<(String, i64)>, AppError> {
        let conn = lock_conn!(self.conn);
        let like = format!("{SHARE_REMOTE_PROVIDER_PREFIX}:%");
        let mut stmt = conn
            .prepare(
                "SELECT provider_id, COALESCE(SUM(
                    input_tokens + output_tokens + cache_read_tokens + cache_creation_tokens
                 ), 0) AS total
                 FROM proxy_request_logs
                 WHERE provider_id LIKE ?1 AND created_at >= ?2 AND status_code < 400
                 GROUP BY provider_id",
            )
            .map_err(|e| AppError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![like, since_epoch], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
            })
            .map_err(|e| AppError::Database(e.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(rows)
    }

    pub fn share_consumed_tokens(&self, since_epoch: i64) -> Result<i64, AppError> {
        let conn = lock_conn!(self.conn);
        let like = format!("{SHARE_REMOTE_PROVIDER_PREFIX}:%");
        let total: i64 = conn
            .query_row(
                "SELECT COALESCE(SUM(
                    input_tokens + output_tokens + cache_read_tokens + cache_creation_tokens
                 ), 0)
                 FROM proxy_request_logs
                 WHERE provider_id LIKE ?1 AND created_at >= ?2 AND status_code < 400",
                params![like, since_epoch],
                |r| r.get(0),
            )
            .map_err(|e| AppError::Database(format!("统计共享网络消费用量失败: {e}")))?;
        Ok(total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn share_network_roundtrip() {
        let db = Database::memory().expect("memory db");
        assert!(db.get_share_network().unwrap().is_none());

        let row = ShareNetworkRow {
            share_id: "AB3F-K7Q2".to_string(),
            share_id_hash: "0123456789abcdef".to_string(),
            role: "creator".to_string(),
            route_preference: "network_first".to_string(),
            relay_addr: None,
            shared_provider_ids: vec!["claude:p1".to_string()],
            quota_scope: "daily".to_string(),
            quota_max_tokens: 100_000,
            quota_per_peer: true,
            node_name: "测试节点".to_string(),
            created_at: 1,
            updated_at: 1,
        };
        db.save_share_network(&row).unwrap();
        let loaded = db.get_share_network().unwrap().expect("network exists");
        assert_eq!(loaded.share_id, "AB3F-K7Q2");
        assert_eq!(loaded.shared_provider_ids, vec!["claude:p1"]);
        assert!(loaded.quota_per_peer);

        db.delete_share_network().unwrap();
        assert!(db.get_share_network().unwrap().is_none());
    }

    #[test]
    fn blocked_peers_roundtrip() {
        let db = Database::memory().expect("memory db");
        assert!(!db.is_share_peer_blocked("peer-a").unwrap());

        db.block_share_peer("peer-a", Some("滥用")).unwrap();
        assert!(db.is_share_peer_blocked("peer-a").unwrap());
        assert_eq!(db.list_share_blocked_peers().unwrap().len(), 1);

        db.unblock_share_peer("peer-a").unwrap();
        assert!(!db.is_share_peer_blocked("peer-a").unwrap());
    }

    #[test]
    fn peer_tokens_aggregation_by_lend_prefix() {
        let db = Database::memory().expect("memory db");
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO proxy_request_logs (
                    request_id, provider_id, app_type, model, input_tokens,
                    output_tokens, cache_read_tokens, cache_creation_tokens,
                    latency_ms, status_code, created_at
                 ) VALUES
                    ('r1', 'sharelend:peer-a:p1', 'claude', 'm', 10, 20, 0, 0, 1, 200, 100),
                    ('r2', 'sharelend:peer-a:p2', 'claude', 'm', 1, 2, 3, 4, 1, 200, 200),
                    ('r3', 'sharelend:peer-b:p1', 'claude', 'm', 100, 0, 0, 0, 1, 200, 100),
                    ('r4', 'local-provider', 'claude', 'm', 999, 0, 0, 0, 1, 200, 100),
                    ('r5', 'share:peer-a', 'claude', 'm', 8, 5, 2, 1, 1, 200, 200),
                    ('r6', 'share:peer-b', 'codex', 'm', 20, 10, 0, 0, 1, 200, 100),
                    ('r7', 'share:peer-b', 'codex', 'm', 100, 100, 0, 0, 1, 500, 200)",
                [],
            )
            .unwrap();
        }
        // peer-a 全时段：10+20 + 1+2+3+4 = 40
        assert_eq!(db.share_peer_tokens_used("peer-a", 0).unwrap(), 40);
        // peer-a 从 epoch 150 起：仅 r2 = 10
        assert_eq!(db.share_peer_tokens_used("peer-a", 150).unwrap(), 10);
        // 全部 peer 聚合
        let all = db.share_all_peers_tokens_used(0).unwrap();
        let map: std::collections::HashMap<_, _> = all.into_iter().collect();
        assert_eq!(map.get("peer-a"), Some(&40));
        assert_eq!(map.get("peer-b"), Some(&100));
        assert!(!map.contains_key("local-provider"));
        // 消费侧全时段：r5 16 + r6 30；失败的 r7 不计入
        assert_eq!(db.share_consumed_tokens(0).unwrap(), 46);
        // 从 epoch 150 起仅 r5
        assert_eq!(db.share_consumed_tokens(150).unwrap(), 16);
    }

    #[test]
    fn remote_provider_tokens_group_by_full_provider_id() {
        let db = Database::memory().expect("memory db");
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO proxy_request_logs (
                    request_id, provider_id, app_type, model, input_tokens,
                    output_tokens, cache_read_tokens, cache_creation_tokens,
                    latency_ms, status_code, created_at
                 ) VALUES
                    ('c1', 'share:peer-a:zhipu', 'codex', 'glm-5.3', 100, 50, 10, 5, 1, 200, 100),
                    ('c2', 'share:peer-a:zhipu', 'codex', 'glm-5.3', 1, 1, 0, 0, 1, 200, 150),
                    ('c3', 'share:peer-a:official', 'codex', 'gpt', 7, 3, 0, 0, 1, 200, 100),
                    ('c4', 'share:peer-b:zhipu', 'codex', 'glm-5.3', 40, 0, 0, 0, 1, 200, 100),
                    ('c5', 'share:peer-b:zhipu', 'codex', 'glm-5.3', 999, 999, 0, 0, 1, 502, 100),
                    ('c6', 'sharelend:peer-b:zhipu', 'codex', 'glm-5.3', 500, 0, 0, 0, 1, 200, 100)",
                [],
            )
            .unwrap();
        }
        let map: std::collections::HashMap<_, _> = db
            .share_remote_provider_tokens_used(0)
            .unwrap()
            .into_iter()
            .collect();
        // 同 (peer, provider) 跨请求累加；失败行(c5)与出借侧行(c6)不计
        assert_eq!(map.get("share:peer-a:zhipu"), Some(&167));
        assert_eq!(map.get("share:peer-a:official"), Some(&10));
        assert_eq!(map.get("share:peer-b:zhipu"), Some(&40));
        assert!(!map.contains_key("sharelend:peer-b:zhipu"));
        // 周期过滤：epoch 150 起 peer-a:zhipu 只剩 c2
        let since: std::collections::HashMap<_, _> = db
            .share_remote_provider_tokens_used(150)
            .unwrap()
            .into_iter()
            .collect();
        assert_eq!(since.get("share:peer-a:zhipu"), Some(&2));
    }
}
