//! SQLite persistence for ordered account groups.
//!
//! This module owns only the group tables.  It deliberately does not know about
//! API keys, request routing, or account deletion; those are coordinated by their
//! respective higher-level services.

use std::fmt;

use rusqlite::{params, Connection, OptionalExtension};

use crate::server::db::Db;
use crate::server::logging;

/// A group member kind stored in `account_group_members.member_type`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MemberType {
    Account,
    Provider,
}

impl MemberType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Account => "account",
            Self::Provider => "provider",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "account" => Some(Self::Account),
            "provider" => Some(Self::Provider),
            _ => None,
        }
    }
}

impl fmt::Display for MemberType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One ordered account-group member.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountGroupMember {
    pub position: i64,
    pub member_type: MemberType,
    pub member_id: String,
}

/// A persisted group and its ordered members.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountGroup {
    pub id: String,
    pub name: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub members: Vec<AccountGroupMember>,
}

/// Storage/service error with the HTTP status exposed by the management API.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountGroupError {
    pub message: String,
    pub status_code: i32,
}

impl AccountGroupError {
    pub fn new(message: impl Into<String>, status_code: i32) -> Self {
        Self {
            message: message.into(),
            status_code,
        }
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(message, 400)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(message, 404)
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(message, 409)
    }

    pub fn storage(message: impl Into<String>) -> Self {
        Self::new(message, 500)
    }
}

impl fmt::Display for AccountGroupError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for AccountGroupError {}

#[derive(Clone, Debug)]
struct RawMember {
    position: i64,
    member_type: String,
    member_id: String,
}

#[derive(Clone, Debug)]
struct RawGroup {
    id: String,
    name: String,
    created_at: i64,
    updated_at: i64,
    members: Vec<RawMember>,
}

impl RawGroup {
    fn into_group(self) -> Result<AccountGroup, AccountGroupError> {
        let mut members = Vec::with_capacity(self.members.len());
        for member in self.members {
            let Some(member_type) = MemberType::parse(&member.member_type) else {
                return Err(AccountGroupError::storage(format!(
                    "账号组 {} 包含未知成员类型 {}",
                    self.id, member.member_type
                )));
            };
            members.push(AccountGroupMember {
                position: member.position,
                member_type,
                member_id: member.member_id,
            });
        }
        Ok(AccountGroup {
            id: self.id,
            name: self.name,
            created_at: self.created_at,
            updated_at: self.updated_at,
            members,
        })
    }
}

pub(crate) fn normalize_name(name: &str) -> Result<(String, String), AccountGroupError> {
    let display = name.trim();
    if display.is_empty() {
        return Err(AccountGroupError::bad_request("账号组名称不能为空"));
    }
    Ok((display.to_string(), display.to_lowercase()))
}

pub(crate) fn normalize_members(
    members: Vec<AccountGroupMember>,
) -> Result<Vec<AccountGroupMember>, AccountGroupError> {
    let mut normalized = Vec::with_capacity(members.len());
    for (position, mut member) in members.into_iter().enumerate() {
        let member_id = member.member_id.trim().to_string();
        if member_id.is_empty() {
            return Err(AccountGroupError::bad_request("账号组成员 id 不能为空"));
        }
        member.position = position as i64;
        member.member_id = member_id;
        if normalized.iter().any(|known: &AccountGroupMember| {
            known.member_type == member.member_type && known.member_id == member.member_id
        }) {
            return Err(AccountGroupError::bad_request("账号组不能重复添加同一成员"));
        }
        normalized.push(member);
    }
    Ok(normalized)
}

fn with_db<T>(
    db: Option<&Db>,
    action: impl FnOnce(&Connection) -> rusqlite::Result<T>,
) -> Result<T, AccountGroupError> {
    let Some(db) = db else {
        return Err(AccountGroupError::storage(
            "账号组数据库不可用（数据库未打开）",
        ));
    };
    db.with(action)
        .ok_or_else(|| AccountGroupError::storage("账号组数据库不可用（连接不可用）"))?
        .map_err(|error| AccountGroupError::storage(format!("账号组数据库操作失败: {error}")))
}

fn with_db_mut<T>(
    db: Option<&Db>,
    action: impl FnOnce(&mut Connection) -> rusqlite::Result<T>,
) -> Result<T, AccountGroupError> {
    let Some(db) = db else {
        return Err(AccountGroupError::storage(
            "账号组数据库不可用（数据库未打开）",
        ));
    };
    db.with_mut(action)
        .ok_or_else(|| AccountGroupError::storage("账号组数据库不可用（连接不可用）"))?
        .map_err(|error| map_sql_error(error))
}

fn map_sql_error(error: rusqlite::Error) -> AccountGroupError {
    let message = error.to_string();
    if message.contains("UNIQUE constraint failed: account_groups.name_key") {
        AccountGroupError::conflict("账号组名称已存在")
    } else {
        AccountGroupError::storage(format!("账号组数据库操作失败: {message}"))
    }
}

fn load_members(conn: &Connection, group_id: &str) -> rusqlite::Result<Vec<RawMember>> {
    let mut statement = conn.prepare(
        "SELECT position, member_type, member_id
         FROM account_group_members
         WHERE group_id = ?1
         ORDER BY position ASC",
    )?;
    let mut rows = statement.query(params![group_id])?;
    let mut members = Vec::new();
    while let Some(row) = rows.next()? {
        members.push(RawMember {
            position: row.get(0)?,
            member_type: row.get(1)?,
            member_id: row.get(2)?,
        });
    }
    Ok(members)
}

fn load_group(conn: &Connection, group_id: &str) -> rusqlite::Result<Option<RawGroup>> {
    let Some((id, name, created_at, updated_at)) = conn
        .query_row(
            "SELECT id, name, created_at, updated_at
             FROM account_groups WHERE id = ?1",
            params![group_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )
        .optional()?
    else {
        return Ok(None);
    };
    Ok(Some(RawGroup {
        id,
        name,
        created_at,
        updated_at,
        members: load_members(conn, group_id)?,
    }))
}

pub(crate) fn list(db: Option<&Db>) -> Result<Vec<AccountGroup>, AccountGroupError> {
    let raw_groups = with_db(db, |conn| {
        let mut statement = conn.prepare(
            "SELECT id, name, created_at, updated_at
             FROM account_groups ORDER BY created_at ASC, id ASC",
        )?;
        let mut rows = statement.query([])?;
        let mut groups = Vec::new();
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            groups.push(RawGroup {
                members: load_members(conn, &id)?,
                id,
                name: row.get(1)?,
                created_at: row.get(2)?,
                updated_at: row.get(3)?,
            });
        }
        Ok(groups)
    })?;
    raw_groups.into_iter().map(RawGroup::into_group).collect()
}

pub(crate) fn get(
    db: Option<&Db>,
    group_id: &str,
) -> Result<Option<AccountGroup>, AccountGroupError> {
    with_db(db, |conn| load_group(conn, group_id))?
        .map(RawGroup::into_group)
        .transpose()
}

pub(crate) fn create(
    db: Option<&Db>,
    name: String,
    name_key: String,
) -> Result<AccountGroup, AccountGroupError> {
    let now = logging::now_ms();
    let created = with_db_mut(db, |conn| {
        let tx = conn.transaction()?;
        let count: i64 =
            tx.query_row("SELECT COUNT(*) FROM account_groups", [], |row| row.get(0))?;
        let mut suffix = count.max(0) as u64;
        let id = loop {
            let candidate = format!("g{now:x}{suffix}");
            let exists: Option<String> = tx
                .query_row(
                    "SELECT id FROM account_groups WHERE id = ?1",
                    params![candidate],
                    |row| row.get(0),
                )
                .optional()?;
            if exists.is_none() {
                break candidate;
            }
            suffix += 1;
        };
        tx.execute(
            "INSERT INTO account_groups (id, name, name_key, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?4)",
            params![id, name, name_key, now],
        )?;
        tx.commit()?;
        Ok((id, now))
    })?;
    Ok(AccountGroup {
        id: created.0,
        name,
        created_at: created.1,
        updated_at: created.1,
        members: Vec::new(),
    })
}

pub(crate) fn rename(
    db: Option<&Db>,
    group_id: &str,
    name: String,
    name_key: String,
) -> Result<AccountGroup, AccountGroupError> {
    let now = logging::now_ms();
    let updated = with_db_mut(db, |conn| {
        let tx = conn.transaction()?;
        let changed = tx.execute(
            "UPDATE account_groups SET name = ?1, name_key = ?2, updated_at = ?3
             WHERE id = ?4",
            params![name, name_key, now, group_id],
        )?;
        if changed == 0 {
            return Ok(None);
        }
        tx.commit()?;
        Ok(Some(()))
    })?;
    if updated.is_none() {
        return Err(AccountGroupError::not_found("账号组不存在"));
    }
    get(db, group_id)?.ok_or_else(|| AccountGroupError::not_found("账号组不存在"))
}

pub(crate) fn replace_members(
    db: Option<&Db>,
    group_id: &str,
    members: Vec<AccountGroupMember>,
) -> Result<AccountGroup, AccountGroupError> {
    let existing =
        get(db, group_id)?.ok_or_else(|| AccountGroupError::not_found("账号组不存在"))?;
    let members = normalize_members(members)?;
    let now = logging::now_ms();
    with_db_mut(db, |conn| {
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM account_group_members WHERE group_id = ?1",
            params![group_id],
        )?;
        for member in &members {
            tx.execute(
                "INSERT INTO account_group_members
                 (group_id, position, member_type, member_id)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    group_id,
                    member.position,
                    member.member_type.as_str(),
                    member.member_id
                ],
            )?;
        }
        tx.execute(
            "UPDATE account_groups SET updated_at = ?1 WHERE id = ?2",
            params![now, group_id],
        )?;
        tx.commit()
    })?;
    Ok(AccountGroup {
        id: existing.id,
        name: existing.name,
        created_at: existing.created_at,
        updated_at: now,
        members,
    })
}

pub(crate) fn delete(db: Option<&Db>, group_id: &str) -> Result<bool, AccountGroupError> {
    with_db_mut(db, |conn| {
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM account_group_members WHERE group_id = ?1",
            params![group_id],
        )?;
        let deleted = tx.execute(
            "DELETE FROM account_groups WHERE id = ?1",
            params![group_id],
        )? > 0;
        tx.commit()?;
        Ok(deleted)
    })
}
