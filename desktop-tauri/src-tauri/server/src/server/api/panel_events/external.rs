use crate::server::db::Db;

pub(super) struct ExternalChanges {
    db: Option<Db>,
    version: Option<i64>,
}

impl ExternalChanges {
    pub(super) fn new(db: Option<Db>) -> Self {
        let mut source = Self { db, version: None };
        source.changed();
        source
    }

    pub(super) fn changed(&mut self) -> bool {
        let Some(db) = &self.db else { return false };
        let version = db.with(|conn| conn.pragma_query_value(None, "data_version", |row| row.get::<_, i64>(0)).ok()).flatten();
        let Some(version) = version else { return false };
        let previous = self.version.replace(version);
        previous.is_some_and(|previous| previous != version)
    }
}
