use super::{read_rows, state::{AccountState, StoredAccount}, AccountStore};

impl AccountStore {
    pub(super) fn read_record(&self, id: &str) -> Option<StoredAccount> {
        let db = self.database()?;
        let raw = {
            let _guard = self.guard();
            db.with(|conn| read_rows::by_id(conn, id)).and_then(Result::ok).flatten()?
        };
        match read_rows::decode(raw) {
            Ok((_, record)) => Some(record),
            Err(error) => {
                super::sql::warn_load_failure("按 id 读取账号", &format!("账号 {id} 读不出来：{}", error.describe()));
                None
            }
        }
    }

    pub(super) fn read_provider(&self, provider: &str) -> Vec<StoredAccount> {
        let Some(db) = self.database() else { return Vec::new() };
        let rows = {
            let _guard = self.guard();
            db.with(|conn| read_rows::provider(conn, provider)).and_then(Result::ok).unwrap_or_default()
        };
        read_rows::decode_many(rows, &format!("读取 {provider} 的账号"))
            .into_iter().map(|(_, record)| record).collect()
    }

    pub(super) fn read_state(&self) -> AccountState {
        let Some(db) = self.database() else { return AccountState::default() };
        let result = {
            let _guard = self.guard();
            db.with(|conn| {
                Ok::<_, rusqlite::Error>((read_rows::all(conn)?, super::sql::load_priority_scope(conn)?))
            }).and_then(Result::ok)
        };
        let Some((rows, priority_scope)) = result else { return AccountState::default() };
        let accounts = read_rows::decode_many(rows, "读取全部账号")
            .into_iter().map(|(_, record)| record).collect();
        AccountState { accounts, priority_scope }
    }
}
