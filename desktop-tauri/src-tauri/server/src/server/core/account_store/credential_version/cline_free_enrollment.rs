//! Free真正新账号的固定首次登记门面。候选材料不是确认许可，也不签发续期/发送资格。
//! 仅原确认的单次worker调用；来源字段私有，无路径、账号ID、JSON或ABI重建入口。

use std::any::Any;
use std::panic::{catch_unwind, AssertUnwindSafe};

use super::{cached_key, sql, AccountStore, FreeEnrollmentInput};
use crate::server::db::{CredentialSource, FreeEnrollmentStorageError, FreeEnrollmentTransaction};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FreeEnrollmentError {
    Unavailable,
    SourceChanged,
    KeyUnavailable,
    InvalidInput,
    InvalidRecord,
    LimitExceeded,
    ExistingAccount,
    IdentityConflict,
    ExistingSlot,
    RefreshRegistrationMissing,
    RecoveryIsolated,
    Expired,
    ClockUnavailable,
    PriorityExhausted,
    RandomUnavailable,
    Storage,
    Interrupted,
    Transaction(FreeEnrollmentStorageError),
}

impl FreeEnrollmentError {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::Unavailable => "cline_free_enrollment_store_unavailable",
            Self::SourceChanged => "cline_free_enrollment_source_changed",
            Self::KeyUnavailable => "cline_free_enrollment_key_unavailable",
            Self::InvalidInput => "cline_free_enrollment_input_invalid",
            Self::InvalidRecord => "cline_free_enrollment_record_invalid",
            Self::LimitExceeded => "cline_free_enrollment_store_limit",
            Self::ExistingAccount => "cline_free_enrollment_existing_account",
            Self::IdentityConflict => "cline_free_enrollment_identity_conflict",
            Self::ExistingSlot => "cline_free_enrollment_existing_slot",
            Self::RefreshRegistrationMissing => "cline_free_refresh_registration_required",
            Self::RecoveryIsolated => "cline_free_enrollment_recovery_isolated",
            Self::Expired => "cline_free_enrollment_material_expired",
            Self::ClockUnavailable => "cline_free_enrollment_clock_unavailable",
            Self::PriorityExhausted => "cline_free_enrollment_priority_exhausted",
            Self::RandomUnavailable => "cline_free_enrollment_random_unavailable",
            Self::Storage => "cline_free_enrollment_storage_failed",
            Self::Interrupted => "cline_free_enrollment_storage_interrupted",
            Self::Transaction(_) => "cline_free_enrollment_transaction_unavailable",
        }
    }

    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Unavailable => "原Free账号数据库不可用，未改用另一份存储首次登记",
            Self::SourceChanged => "原Free数据库来源已变化，未捕获新来源继续本次保存",
            Self::KeyUnavailable => "账号加密密钥缓存不可用，未在存储锁内加载或生成密钥",
            Self::InvalidInput => "本次Free完整材料与固定身份不一致，未从旧行补齐",
            Self::InvalidRecord => "旧Free账号或独立槽格式异常，未跳过坏行后宣称真正新账号",
            Self::LimitExceeded => "Free首次登记超过本家有界核对限额，未发布部分结果",
            Self::ExistingAccount => "本次Free身份或材料已在旧账号中存在，本入口只接受真正新账号",
            Self::IdentityConflict => "Free身份或固定账号主键冲突，未换一个ID绕过旧账号",
            Self::ExistingSlot => "同Free身份或账号已有登记槽，未覆盖、清除或重新计次",
            Self::RefreshRegistrationMissing => "Cline Free 当前账号没有受控续期登记，未建立新登记或发送换证请求",
            Self::RecoveryIsolated => "Free宿主版本不属于当前恢复代次，未重建旧版本恢复资格",
            Self::Expired => "本次Free材料在实际保存前已到期，未补写默认期限或继续保存",
            Self::ClockUnavailable => "实际保存时间无法确认，未把未知时间当作仍有效",
            Self::PriorityExhausted => "全局账号优先级已满，未调整其他账号或覆盖占位",
            Self::RandomUnavailable => "随机源未能生成独立新代次，未使用默认值或重试",
            Self::Storage => "Free首次登记存储失败，未补查较新数据库或自动再次保存",
            Self::Interrupted => "Free首次登记存储被中断，请按真实保存结果判断，未自动重试",
            Self::Transaction(_) => "原Free数据库耐久事务不可用，未取得首次登记资格",
        }
    }
}

#[must_use]
pub(crate) struct FreeEnrollmentOrigin {
    store: AccountStore,
    source: CredentialSource,
}

/// 仅真实COMMIT成功后交付。本回执不含两令牌、可重用来源或再次登记能力。
#[must_use]
pub(crate) struct FreeEnrollmentReceipt {
    account: String,
}

impl FreeEnrollmentReceipt {
    pub(crate) fn account(&self) -> &str {
        &self.account
    }

    /// 本次首次Ready登记事实；不表示JWT签名/上游验证、令牌未消费或连接仍可用。
    pub(crate) fn ready_registered(&self) -> bool {
        true
    }
}

#[must_use]
pub(crate) enum FreeEnrollmentCommitOutcome {
    NotCommitted(FreeEnrollmentError),
    CommitUnknown {
        storage_closed: bool,
    },
    Committed {
        receipt: FreeEnrollmentReceipt,
        storage_closed: bool,
    },
}

impl AccountStore {
    /// 只固定捕获原Store/Db的私有进程来源；不读账号/kv、不写版本、不建槽。
    pub(crate) fn prepare_cline_free_enrollment_origin(
        &self,
    ) -> Result<FreeEnrollmentOrigin, FreeEnrollmentError> {
        let db = self.database().ok_or(FreeEnrollmentError::Unavailable)?;
        let source = db
            .capture_cline_free_enrollment_source()
            .map_err(|_| FreeEnrollmentError::Unavailable)?;
        Ok(FreeEnrollmentOrigin {
            store: self.clone(),
            source,
        })
    }
}

impl FreeEnrollmentOrigin {
    /// 确认许可由外层原确认状态负责；本函数只执行一次固定存储，不接任意callback。
    /// Input所有权持至实际事务及收尾完成；未知结果不得补查、重试或降格为未写入。
    pub(crate) fn commit_initial(self, input: FreeEnrollmentInput) -> FreeEnrollmentCommitOutcome {
        let key = match cached_key() {
            Ok(key) => key,
            Err(_) => {
                return FreeEnrollmentCommitOutcome::NotCommitted(
                    FreeEnrollmentError::KeyUnavailable,
                )
            }
        };
        let mut retained = sql::credential_version::cline_free_enrollment::RetainedRows::new();
        let mut action_unwind: Option<Box<dyn Any + Send>> = None;
        let mut entered = false;
        let mut commit = None;
        let mut candidate = None;
        let attempt = catch_unwind(AssertUnwindSafe(|| {
            let _guard = self.store.guard();
            let db = self.database()?;
            Ok::<_, FreeEnrollmentError>(db.cline_free_enrollment_transaction_with_fact(
                &self.source, &mut commit,
                |conn| {
                    entered = true;
                    match catch_unwind(AssertUnwindSafe(|| {
                        sql::credential_version::cline_free_enrollment::commit(
                            conn,
                            &key,
                            &input,
                            &mut retained,
                        )
                    })) {
                        Ok(Ok(account)) => { candidate = Some(account); Ok(()) }
                        Ok(Err(error)) => Err(error),
                        Err(payload) => {
                            action_unwind = Some(payload);
                            Err(FreeEnrollmentError::Interrupted)
                        }
                    }
                },
            ))
        }));
        let mut deferred = None;
        let outcome = match attempt {
            Err(payload) => {
                deferred = Some(payload);
                if commit == Some(true) {
                    match candidate.take() {
                        Some(account) => FreeEnrollmentCommitOutcome::Committed {
                            receipt: FreeEnrollmentReceipt { account }, storage_closed: false,
                        },
                        None => FreeEnrollmentCommitOutcome::CommitUnknown { storage_closed: false },
                    }
                } else if entered {
                    FreeEnrollmentCommitOutcome::CommitUnknown {
                        storage_closed: false,
                    }
                } else {
                    FreeEnrollmentCommitOutcome::NotCommitted(FreeEnrollmentError::Interrupted)
                }
            }
            Ok(Err(error)) => FreeEnrollmentCommitOutcome::NotCommitted(error),
            Ok(Ok(FreeEnrollmentTransaction::NotEntered {
                error,
                storage_closed: true,
            })) => FreeEnrollmentCommitOutcome::NotCommitted(
                if error == FreeEnrollmentStorageError::SourceChanged {
                    FreeEnrollmentError::SourceChanged
                } else {
                    FreeEnrollmentError::Transaction(error)
                },
            ),
            Ok(Ok(FreeEnrollmentTransaction::Rejected {
                error,
                storage_closed: true,
            })) => FreeEnrollmentCommitOutcome::NotCommitted(error),
            Ok(Ok(FreeEnrollmentTransaction::NotEntered {
                storage_closed: false,
                ..
            }))
            | Ok(Ok(FreeEnrollmentTransaction::Rejected {
                storage_closed: false,
                ..
            })) => FreeEnrollmentCommitOutcome::CommitUnknown {
                storage_closed: false,
            },
            Ok(Ok(FreeEnrollmentTransaction::CommitUnconfirmed { storage_closed })) => {
                FreeEnrollmentCommitOutcome::CommitUnknown { storage_closed }
            }
            Ok(Ok(FreeEnrollmentTransaction::Committed { storage_closed, .. })) => match candidate.take() {
                Some(account) => FreeEnrollmentCommitOutcome::Committed {
                    receipt: FreeEnrollmentReceipt { account }, storage_closed,
                },
                None => FreeEnrollmentCommitOutcome::CommitUnknown { storage_closed },
            },
        };
        // 包括解析失败时的中间明文与panic载荷，均在原账号/Db锁退出后清理。
        drop(candidate);
        drop(retained);
        drop([action_unwind, deferred]);
        outcome
    }

    fn database(&self) -> Result<&crate::server::db::Db, FreeEnrollmentError> {
        self.store
            .database()
            .ok_or(FreeEnrollmentError::Unavailable)
    }
}
