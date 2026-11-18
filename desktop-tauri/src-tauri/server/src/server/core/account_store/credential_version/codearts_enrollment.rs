//! CodeArts首次登记的原AccountStore/Db保存门面。Input是候选，不是用户批准或发送许可。
//! 仅原confirm的单次worker应调用；无ABI/serde入口、不主动纳管旧账号、不调用刷新端点。

use std::any::Any;
use std::panic::{catch_unwind, AssertUnwindSafe};

use super::{cached_key, codearts_enrollment_input::EnrollmentInput, sql, AccountStore};
use crate::server::db::{CodeArtsStorageError, CodeArtsTransaction, CredentialSource};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EnrollmentError {
    Unavailable,
    SourceChanged,
    KeyUnavailable,
    AccountBusy,
    InvalidRecord,
    LimitExceeded,
    AmbiguousIdentity,
    IdentityConflict,
    ExistingSlot,
    RecoveryIsolated,
    RequiresNewAuthorization,
    Expired,
    PriorityExhausted,
    VersionExhausted,
    RandomUnavailable,
    Storage,
    Transaction(CodeArtsStorageError),
}

impl EnrollmentError {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::Unavailable => "codearts_enrollment_store_unavailable",
            Self::SourceChanged => "codearts_enrollment_database_changed",
            Self::KeyUnavailable => "codearts_enrollment_key_unavailable",
            Self::AccountBusy => "codearts_enrollment_account_busy",
            Self::InvalidRecord => "codearts_enrollment_record_invalid",
            Self::LimitExceeded => "codearts_enrollment_store_limit",
            Self::AmbiguousIdentity => "codearts_enrollment_ambiguous_identity",
            Self::IdentityConflict => "codearts_enrollment_identity_conflict",
            Self::ExistingSlot => "codearts_enrollment_slot_exists",
            Self::RecoveryIsolated => "codearts_enrollment_recovery_isolated",
            Self::RequiresNewAuthorization => "codearts_enrollment_new_authorization_required",
            Self::Expired => "codearts_enrollment_material_expired",
            Self::PriorityExhausted => "codearts_enrollment_priority_exhausted",
            Self::VersionExhausted => "codearts_enrollment_version_exhausted",
            Self::RandomUnavailable => "codearts_enrollment_random_unavailable",
            Self::Storage => "codearts_enrollment_storage_failed",
            Self::Transaction(_) => "codearts_enrollment_transaction_unavailable",
        }
    }

    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Unavailable => "原账号数据库不可用，未改用另一份存储继续首次登记",
            Self::SourceChanged => "原数据库来源已变化，本次首次登记已拒绝，请重新准备",
            Self::KeyUnavailable => "账号加密密钥缓存不可用，未在存储锁内加载或生成密钥",
            Self::AccountBusy => "原账号保存正忙，本次未进入首次登记，不自动重试",
            Self::InvalidRecord => "CodeArts原账号或单槽存储格式异常，未忽略坏记录或覆盖",
            Self::LimitExceeded => "CodeArts首次登记超过有界存储或账号核对限额，未发布部分结果",
            Self::AmbiguousIdentity => "多条CodeArts账号匹配本次完整身份，未合并或新建另一账号绕过",
            Self::IdentityConflict => "CodeArts原账号身份与本次材料冲突，未换一个账号ID继续登记",
            Self::ExistingSlot => "目标账号已经存在登记槽，不覆盖、不重新计次或按超时解封",
            Self::RecoveryIsolated => {
                "目标CodeArts账号属于旧恢复代次，需要可信新授权，未重建版本掩盖隔离"
            }
            Self::RequiresNewAuthorization => {
                "目标CodeArts账号已要求重新授权，手工再次确认不能恢复旧资格"
            }
            Self::Expired => "本次完整材料在实际保存前已到期，未沿用旧材料或推测新的期限",
            Self::PriorityExhausted => "全局账号优先级号段已满，未调整其他账号或覆盖原记录",
            Self::VersionExhausted => "CodeArts账号版本无法继续推进，未回退代次",
            Self::RandomUnavailable => "系统随机源未能建立安全的新代次，未使用默认值或重试登记",
            Self::Storage => "首次登记的存储动作失败，请以本次提交及收尾状态判断，不自动再次保存",
            Self::Transaction(_) => "原数据库耐久事务尚不可用，本次未获得新的登记资格",
        }
    }
}

/// 原对象/私有Arc的固定捕获，不包含账号、JSON、路径或可自行重建的代次字段。
#[must_use]
pub(crate) struct EnrollmentOrigin {
    store: AccountStore,
    source: CredentialSource,
}

/// 只在真实COMMIT成功后交付；没有凭据、可重用读来源或再次登记入口。
#[must_use]
pub(crate) struct EnrollmentReceipt {
    account: String,
}

impl EnrollmentReceipt {
    pub(crate) fn account(&self) -> &str {
        &self.account
    }
    /// 是本次事务首次登记的事实，不表示上游验证、令牌未消费或当前连接仍可使用。
    pub(crate) fn ready_registered(&self) -> bool {
        true
    }
}

#[must_use]
pub(crate) enum EnrollmentCommitOutcome {
    NotEntered(EnrollmentError),
    Rejected {
        error: EnrollmentError,
        storage_closed: bool,
    },
    CommitUnknown {
        storage_closed: bool,
    },
    Committed {
        receipt: EnrollmentReceipt,
        storage_closed: bool,
    },
}

impl AccountStore {
    /// 仅捕获原Store/Db真实进程代次：不读账号/kv、不写版本、不建槽、不构成用户许可。
    pub(crate) fn prepare_codearts_enrollment_origin(
        &self,
    ) -> Result<EnrollmentOrigin, EnrollmentError> {
        let db = self.database().ok_or(EnrollmentError::Unavailable)?;
        let source = db
            .capture_codearts_enrollment_source()
            .map_err(|_| EnrollmentError::Unavailable)?;
        Ok(EnrollmentOrigin {
            store: self.clone(),
            source,
        })
    }
}

impl EnrollmentOrigin {
    /// 保存批准由外层原confirm一次状态负责；本固定入口只保存候选，不能从ABI/JSON批准。
    /// Input持有到实际事务/收尾结束。未知结果不补查较新库，不重试，不回退为“尚未写入”。
    pub(crate) fn commit_initial(self, input: EnrollmentInput) -> EnrollmentCommitOutcome {
        self.commit(input, false, None)
    }

    pub(crate) fn commit_new_login(self, login: crate::server::core::plugin_host::wasm::codearts_login::Completed,
        proxy: Option<&serde_json::Value>,
    ) -> EnrollmentCommitOutcome {
        let proxy = match proxy.map(crate::server::core::proxies::normalize_account_proxy).transpose() {
            Ok(proxy) => proxy.flatten(),
            Err(_) => return EnrollmentCommitOutcome::NotEntered(EnrollmentError::InvalidRecord),
        };
        self.commit(login.into_input(), true, proxy)
    }

    fn commit(self, input: EnrollmentInput, new_login: bool, proxy: Option<serde_json::Value>) -> EnrollmentCommitOutcome {
        let key = match cached_key() {
            Ok(key) => key,
            Err(_) => return EnrollmentCommitOutcome::NotEntered(EnrollmentError::KeyUnavailable),
        };
        // SQL构造的原行/新行明文由锁外所有者持有；异常载荷也只能在所有锁释放后析构。
        let mut rows = sql::credential_version::codearts_enrollment::RetainedRows::new();
        let mut action_unwind: Option<Box<dyn Any + Send>> = None;
        let mut entered = false;
        let mut commit = None;
        let mut candidate = None;
        let attempt = catch_unwind(AssertUnwindSafe(|| {
            let _guard = self.store.guard();
            let db = self.store.database().ok_or(EnrollmentError::Unavailable)?;
            Ok::<_, EnrollmentError>(db.codearts_slot_transaction_with_fact(&self.source, &mut commit, |conn| {
                entered = true;
                let result = catch_unwind(AssertUnwindSafe(|| {
                    sql::credential_version::codearts_enrollment::commit(
                        conn, &key, &input, &mut rows, new_login, proxy.as_ref(),
                    )
                }));
                match result {
                    Ok(Ok(account)) => { candidate = Some(account); Ok(()) }
                    Ok(Err(error)) => Err(error),
                    Err(payload) => {
                        action_unwind = Some(payload);
                        Err(EnrollmentError::Storage)
                    }
                }
            }))
        }));
        let mut deferred = None;
        let outcome = match attempt {
            Err(payload) => {
                deferred = Some(payload);
                if commit == Some(true) {
                    match candidate.take() {
                        Some(account) => EnrollmentCommitOutcome::Committed {
                            receipt: EnrollmentReceipt { account }, storage_closed: false,
                        },
                        None => EnrollmentCommitOutcome::CommitUnknown { storage_closed: false },
                    }
                } else if entered {
                    EnrollmentCommitOutcome::CommitUnknown {
                        storage_closed: false,
                    }
                } else {
                    EnrollmentCommitOutcome::NotEntered(EnrollmentError::Unavailable)
                }
            }
            Ok(Err(error)) => EnrollmentCommitOutcome::NotEntered(error),
            Ok(Ok(CodeArtsTransaction::NotEntered {
                error,
                storage_closed,
            })) => {
                if storage_closed {
                    EnrollmentCommitOutcome::NotEntered(
                        if error == CodeArtsStorageError::SourceChanged {
                            EnrollmentError::SourceChanged
                        } else {
                            EnrollmentError::Transaction(error)
                        },
                    )
                } else {
                    EnrollmentCommitOutcome::CommitUnknown {
                        storage_closed: false,
                    }
                }
            }
            Ok(Ok(CodeArtsTransaction::Rejected {
                error,
                storage_closed,
            })) => EnrollmentCommitOutcome::Rejected {
                error,
                storage_closed,
            },
            Ok(Ok(CodeArtsTransaction::CommitUnconfirmed { storage_closed })) => {
                EnrollmentCommitOutcome::CommitUnknown { storage_closed }
            }
            Ok(Ok(CodeArtsTransaction::Committed { storage_closed, .. })) => match candidate.take() {
                Some(account) => EnrollmentCommitOutcome::Committed {
                    receipt: EnrollmentReceipt { account }, storage_closed,
                },
                None => EnrollmentCommitOutcome::CommitUnknown { storage_closed },
            },
        };
        drop(candidate);
        drop(rows);
        drop([action_unwind, deferred]);
        outcome
    }
}
