//! 原生菜单与系统对话框的进程级界面语言。
//!
//! 网页词典不覆盖系统托盘和原生对话框，因此这里单独保存一份轻量、完整的六语映射。
//! 语言状态属于整个进程，而不属于某个 WebView：轻量模式销毁并重建主窗口后，
//! 托盘和之后打开的原生对话框仍使用用户最后选择的语言。

use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub(crate) enum Locale {
    ZhHans = 0,
    ZhHant = 1,
    En = 2,
    Ja = 3,
    Ko = 4,
    PtBr = 5,
}

impl Locale {
    #[cfg(test)]
    const ALL: [Self; 6] = [
        Self::ZhHans,
        Self::ZhHant,
        Self::En,
        Self::Ja,
        Self::Ko,
        Self::PtBr,
    ];

    fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::ZhHans,
            1 => Self::ZhHant,
            2 => Self::En,
            3 => Self::Ja,
            4 => Self::Ko,
            5 => Self::PtBr,
            _ => Self::En,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TextKey {
    TrayShowMain,
    TrayCheckUpdate,
    TrayQuit,
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    CompatibilityRestart,
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    CompatibilityRestartMessage,
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    CompatibilityRestartFailed,
    StartupFailureTitle,
    StartupFailureMessage,
    ExitFailureTitle,
    ExitCleanupFailed,
    ExitActionFailed,
    ExitInterrupted,
    ExitConflict,
    Ok,
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    Cancel,
    ExportLogs,
    SelectFolder,
    SelectClientConfigFile,
    ExportAccounts,
    ImportAccounts,
    BackupDatabase,
    RestoreDatabase,
    ExportAccountGroup,
    SelectClientProgram,
    SaveImage,
    AllFiles,
    ConfigFiles,
    ClientProgram,
    Images,
    EncryptedBackup,
    DatabaseBackup,
}

impl TextKey {
    #[cfg(test)]
    const ALL: [Self; 31] = [
        Self::TrayShowMain,
        Self::TrayCheckUpdate,
        Self::TrayQuit,
        Self::CompatibilityRestart,
        Self::CompatibilityRestartMessage,
        Self::CompatibilityRestartFailed,
        Self::StartupFailureTitle,
        Self::StartupFailureMessage,
        Self::ExitFailureTitle,
        Self::ExitCleanupFailed,
        Self::ExitActionFailed,
        Self::ExitInterrupted,
        Self::ExitConflict,
        Self::Ok,
        Self::Cancel,
        Self::ExportLogs,
        Self::SelectFolder,
        Self::SelectClientConfigFile,
        Self::ExportAccounts,
        Self::ImportAccounts,
        Self::BackupDatabase,
        Self::RestoreDatabase,
        Self::ExportAccountGroup,
        Self::SelectClientProgram,
        Self::SaveImage,
        Self::AllFiles,
        Self::ConfigFiles,
        Self::ClientProgram,
        Self::Images,
        Self::EncryptedBackup,
        Self::DatabaseBackup,
    ];
}

static CURRENT_LOCALE: AtomicU8 = AtomicU8::new(Locale::En as u8);

/// 在创建任何窗口、托盘或原生对话框之前调用。
pub(crate) fn initialize_from_system() {
    let system = system_locale_name().unwrap_or_default();
    CURRENT_LOCALE.store(normalize_locale(&system) as u8, Ordering::Release);
}

pub(crate) fn current() -> Locale {
    Locale::from_u8(CURRENT_LOCALE.load(Ordering::Acquire))
}

/// 接受 BCP 47、POSIX locale 以及常见的大小写/分隔符变体；未知值回退英语。
pub(crate) fn normalize_locale(input: &str) -> Locale {
    let normalized = input.trim().replace('_', "-");
    let normalized = normalized
        .split(['.', '@'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let mut parts = normalized.split('-');
    match parts.next().unwrap_or_default() {
        "zh" => {
            if parts.any(|part| matches!(part, "hant" | "cht" | "tw" | "hk" | "mo")) {
                Locale::ZhHant
            } else {
                Locale::ZhHans
            }
        }
        "en" => Locale::En,
        "ja" => Locale::Ja,
        "ko" => Locale::Ko,
        // 目前只提供巴西葡萄牙语；其他葡萄牙语区域也使用这一组完整译文。
        "pt" => Locale::PtBr,
        _ => Locale::En,
    }
}

pub(crate) fn text(key: TextKey) -> &'static str {
    text_for(current(), key)
}

/// 一次托盘菜单重建共用一个 locale 快照，避免并发切换时同一菜单混入两种语言。
pub(crate) fn text_for(locale: Locale, key: TextKey) -> &'static str {
    let values = match key {
        TextKey::TrayShowMain => [
            "显示主窗口",
            "顯示主視窗",
            "Show Main Window",
            "メインウィンドウを表示",
            "메인 창 표시",
            "Mostrar janela principal",
        ],
        TextKey::TrayCheckUpdate => [
            "检查更新",
            "檢查更新",
            "Check for Updates",
            "更新を確認",
            "업데이트 확인",
            "Verificar atualizações",
        ],
        TextKey::TrayQuit => ["退出", "結束", "Quit", "終了", "종료", "Sair"],
        TextKey::CompatibilityRestart => [
            "以兼容模式重启",
            "以相容模式重新啟動",
            "Restart in Compatibility Mode",
            "互換モードで再起動",
            "호환 모드로 다시 시작",
            "Reiniciar em modo de compatibilidade",
        ],
        TextKey::CompatibilityRestartMessage => [
            "关闭 Agent2API 的硬件加速并重启，可能解决白屏、画面不更新或点击无响应。此选择会保存到本机，下次启动继续使用兼容模式；可在设置中重新开启。重启期间网关会短暂中断。",
            "關閉 Agent2API 的硬體加速並重新啟動，可能解決白屏、畫面不更新或點擊無回應。此選擇會儲存到本機，下次啟動繼續使用相容模式；可在設定中重新開啟。重新啟動期間閘道會短暫中斷。",
            "Disable hardware acceleration for Agent2API and restart. This may resolve a blank screen, stale rendering, or unresponsive clicks. This choice is saved on this device and compatibility mode will remain enabled on the next launch; you can re-enable hardware acceleration in Settings. The gateway will be briefly interrupted during the restart.",
            "Agent2API のハードウェア アクセラレーションを無効にして再起動します。画面が真っ白になる、表示が更新されない、クリックに反応しないなどの問題が解決する場合があります。この選択はこのデバイスに保存され、次回起動時も互換モードが維持されます。設定でハードウェア アクセラレーションを再度有効にできます。再起動中はゲートウェイが一時的に中断します。",
            "Agent2API의 하드웨어 가속을 끄고 다시 시작합니다. 흰 화면, 화면이 갱신되지 않거나 클릭에 응답하지 않는 문제를 해결할 수 있습니다. 이 선택은 이 기기에 저장되어 다음 시작에도 호환 모드가 유지되며, 설정에서 하드웨어 가속을 다시 켤 수 있습니다. 다시 시작하는 동안 게이트웨이가 잠시 중단됩니다.",
            "Desative a aceleração de hardware do Agent2API e reinicie. Isso pode resolver tela em branco, imagem sem atualização ou cliques sem resposta. A escolha será salva neste dispositivo e o modo de compatibilidade continuará ativo na próxima inicialização; você pode reativar a aceleração em Configurações. O gateway será interrompido brevemente durante a reinicialização.",
        ],
        TextKey::CompatibilityRestartFailed => [
            "无法以兼容模式重启",
            "無法以相容模式重新啟動",
            "Unable to Restart in Compatibility Mode",
            "互換モードで再起動できません",
            "호환 모드로 다시 시작할 수 없음",
            "Não foi possível reiniciar em modo de compatibilidade",
        ],
        TextKey::StartupFailureTitle => [
            "Agent2API 无法启动",
            "Agent2API 無法啟動",
            "Agent2API Cannot Start",
            "Agent2API を起動できません",
            "Agent2API를 시작할 수 없음",
            "Não foi possível iniciar o Agent2API",
        ],
        TextKey::StartupFailureMessage => [
            "Agent2API 启动已中止：{error}\n\n本次未改动、未删除任何旧数据，也未创建新配置目录。排除原因（磁盘空间、文件占用、权限）后重新打开本程序即可自动重试迁移。",
            "Agent2API 啟動已中止：{error}\n\n本次未改動、未刪除任何舊資料，也未建立新設定目錄。排除原因（磁碟空間、檔案佔用、權限）後重新開啟本程式即可自動重試遷移。",
            "Agent2API startup was aborted: {error}\n\nNo existing data was changed or deleted, and no new configuration directory was created. Resolve the cause (disk space, file locks, or permissions), then reopen the application to retry migration automatically.",
            "Agent2API の起動を中止しました：{error}\n\n既存のデータの変更・削除は行わず、新しい設定ディレクトリも作成していません。原因（ディスク容量、ファイルの占有、権限）を解決してからアプリを再度開くと、移行が自動的に再試行されます。",
            "Agent2API 시작이 중단되었습니다: {error}\n\n기존 데이터를 변경하거나 삭제하지 않았으며 새 구성 디렉터리도 만들지 않았습니다. 원인(디스크 공간, 파일 사용 중, 권한)을 해결한 후 프로그램을 다시 열면 마이그레이션이 자동으로 다시 시도됩니다.",
            "A inicialização do Agent2API foi interrompida: {error}\n\nNenhum dado existente foi alterado ou excluído e nenhum novo diretório de configuração foi criado. Resolva a causa (espaço em disco, arquivos em uso ou permissões) e abra o programa novamente para tentar a migração automaticamente.",
        ],
        TextKey::ExitFailureTitle => [
            "Agent2API 未完成退出操作",
            "Agent2API 未完成結束操作",
            "Agent2API Could Not Finish Exiting",
            "Agent2API の終了処理が完了していません",
            "Agent2API 종료 작업을 완료하지 못함",
            "O Agent2API não concluiu a operação de saída",
        ],
        TextKey::ExitCleanupFailed => [
            "未能确认本地网关和插件已安全停止，已取消自动退出、重启或安装。程序和托盘将保持打开；请查看运行日志。不会自动重试或强制终止。",
            "未能確認本機閘道和外掛已安全停止，已取消自動結束、重新啟動或安裝。程式和系統匣將保持開啟；請查看執行日誌。不會自動重試或強制終止。",
            "The local gateway and plugins could not be confirmed to have stopped safely. Automatic quit, restart, or installation was canceled. The application and tray icon will remain open; check the runtime logs. No automatic retry or forced termination will occur.",
            "ローカルゲートウェイとプラグインが安全に停止したことを確認できないため、自動終了、再起動、またはインストールをキャンセルしました。アプリとトレイアイコンは開いたままになります。実行ログを確認してください。自動再試行や強制終了は行われません。",
            "로컬 게이트웨이와 플러그인이 안전하게 중지되었는지 확인할 수 없어 자동 종료, 다시 시작 또는 설치를 취소했습니다. 프로그램과 트레이 아이콘은 열린 상태로 유지됩니다. 실행 로그를 확인하세요. 자동으로 다시 시도하거나 강제 종료하지 않습니다.",
            "Não foi possível confirmar que o gateway local e os plugins foram encerrados com segurança. A saída, reinicialização ou instalação automática foi cancelada. O aplicativo e o ícone da bandeja permanecerão abertos; consulte os logs de execução. Não haverá nova tentativa automática nem encerramento forçado.",
        ],
        TextKey::ExitActionFailed => [
            "最终操作未能完成，程序保持打开。后台服务不会自动重新启动；请查看运行日志确认状态。",
            "最終操作未能完成，程式保持開啟。背景服務不會自動重新啟動；請查看執行日誌確認狀態。",
            "The final action could not be completed, and the application remains open. Background services will not restart automatically; check the runtime logs to confirm their status.",
            "最終操作を完了できなかったため、アプリは開いたままです。バックグラウンドサービスは自動的に再起動しません。実行ログで状態を確認してください。",
            "최종 작업을 완료하지 못해 프로그램이 열린 상태로 유지됩니다. 백그라운드 서비스는 자동으로 다시 시작되지 않습니다. 실행 로그에서 상태를 확인하세요.",
            "A ação final não pôde ser concluída e o aplicativo permanece aberto. Os serviços em segundo plano não serão reiniciados automaticamente; consulte os logs de execução para confirmar o estado.",
        ],
        TextKey::ExitInterrupted => [
            "退出操作在确认完成前中断，当前结果未知。程序不会自动再次退出、重启或运行安装器。",
            "結束操作在確認完成前中斷，目前結果未知。程式不會自動再次結束、重新啟動或執行安裝程式。",
            "The exit operation was interrupted before completion could be confirmed, and its outcome is unknown. The application will not automatically quit, restart, or run the installer again.",
            "終了処理は完了を確認する前に中断され、結果は不明です。アプリが自動的に再度終了、再起動、またはインストーラーを実行することはありません。",
            "종료 작업이 완료를 확인하기 전에 중단되어 현재 결과를 알 수 없습니다. 프로그램이 자동으로 다시 종료되거나 다시 시작되거나 설치 관리자를 실행하지 않습니다.",
            "A operação de saída foi interrompida antes que sua conclusão pudesse ser confirmada, e o resultado é desconhecido. O aplicativo não sairá, reiniciará nem executará o instalador novamente de forma automática.",
        ],
        TextKey::ExitConflict => [
            "已有另一项退出、重启或安装请求，未覆盖原请求，也未执行新的操作。",
            "已有另一項結束、重新啟動或安裝請求，未覆蓋原請求，也未執行新的操作。",
            "Another quit, restart, or installation request is already in progress. The original request was not replaced, and no new action was performed.",
            "別の終了、再起動、またはインストール要求がすでに進行中です。元の要求は置き換えられず、新しい操作も実行されませんでした。",
            "다른 종료, 다시 시작 또는 설치 요청이 이미 진행 중입니다. 기존 요청을 대체하지 않았으며 새 작업도 실행하지 않았습니다.",
            "Já há outra solicitação de saída, reinicialização ou instalação em andamento. A solicitação original não foi substituída e nenhuma nova ação foi executada.",
        ],
        TextKey::Ok => ["确定", "確定", "OK", "OK", "확인", "OK"],
        TextKey::Cancel => ["取消", "取消", "Cancel", "キャンセル", "취소", "Cancelar"],
        TextKey::ExportLogs => [
            "导出运行日志",
            "匯出執行日誌",
            "Export Runtime Logs",
            "実行ログのエクスポート",
            "실행 로그 내보내기",
            "Exportar logs de execução",
        ],
        TextKey::SelectFolder => [
            "选择文件夹",
            "選擇資料夾",
            "Select Folder",
            "フォルダーを選択",
            "폴더 선택",
            "Selecionar pasta",
        ],
        TextKey::SelectClientConfigFile => [
            "选择客户端配置文件",
            "選擇用戶端設定檔",
            "Select Client Configuration File",
            "クライアント設定ファイルを選択",
            "클라이언트 구성 파일 선택",
            "Selecionar arquivo de configuração do cliente",
        ],
        TextKey::ExportAccounts => [
            "导出账号",
            "匯出帳號",
            "Export Accounts",
            "アカウントのエクスポート",
            "계정 내보내기",
            "Exportar contas",
        ],
        TextKey::ImportAccounts => [
            "导入账号（可多选）",
            "匯入帳號（可多選）",
            "Import Accounts (Multiple Selection)",
            "アカウントのインポート（複数選択可）",
            "계정 가져오기(여러 개 선택 가능)",
            "Importar contas (seleção múltipla)",
        ],
        TextKey::BackupDatabase => [
            "整库备份",
            "完整資料庫備份",
            "Back Up Database",
            "データベースの完全バックアップ",
            "전체 데이터베이스 백업",
            "Fazer backup completo do banco de dados",
        ],
        TextKey::RestoreDatabase => [
            "整库恢复",
            "完整資料庫還原",
            "Restore Database",
            "データベースの完全復元",
            "전체 데이터베이스 복원",
            "Restaurar banco de dados completo",
        ],
        TextKey::ExportAccountGroup => [
            "导出本组账号：选择保存文件夹",
            "匯出此群組帳號：選擇儲存資料夾",
            "Export This Account Group: Choose Destination Folder",
            "このグループのアカウントをエクスポート：保存先フォルダーを選択",
            "이 그룹의 계정 내보내기: 저장할 폴더 선택",
            "Exportar este grupo de contas: selecionar pasta de destino",
        ],
        TextKey::SelectClientProgram => [
            "选择客户端程序",
            "選擇用戶端程式",
            "Select Client Application",
            "クライアントアプリケーションを選択",
            "클라이언트 프로그램 선택",
            "Selecionar aplicativo cliente",
        ],
        TextKey::SaveImage => [
            "保存图片",
            "儲存圖片",
            "Save Image",
            "画像を保存",
            "이미지 저장",
            "Salvar imagem",
        ],
        TextKey::AllFiles => [
            "全部文件",
            "所有檔案",
            "All Files",
            "すべてのファイル",
            "모든 파일",
            "Todos os arquivos",
        ],
        TextKey::ConfigFiles => [
            "配置文件",
            "設定檔",
            "Configuration Files",
            "設定ファイル",
            "구성 파일",
            "Arquivos de configuração",
        ],
        TextKey::ClientProgram => [
            "客户端程序",
            "用戶端程式",
            "Client Application",
            "クライアントアプリケーション",
            "클라이언트 프로그램",
            "Aplicativo cliente",
        ],
        TextKey::Images => ["图片", "圖片", "Images", "画像", "이미지", "Imagens"],
        TextKey::EncryptedBackup => [
            "Agent2API 加密备份",
            "Agent2API 加密備份",
            "Agent2API Encrypted Backup",
            "Agent2API 暗号化バックアップ",
            "Agent2API 암호화 백업",
            "Backup criptografado do Agent2API",
        ],
        TextKey::DatabaseBackup => [
            "Agent2API 备份",
            "Agent2API 備份",
            "Agent2API Backup",
            "Agent2API バックアップ",
            "Agent2API 백업",
            "Backup do Agent2API",
        ],
    };
    values[locale as usize]
}

pub(crate) fn startup_failure_message(error: &str) -> String {
    startup_failure_message_for(current(), error)
}

fn startup_failure_message_for(locale: Locale, error: &str) -> String {
    text_for(locale, TextKey::StartupFailureMessage).replace("{error}", error)
}

/// 前端在词典初始化后以及每次切换语言时调用，立即刷新已经创建的托盘菜单。
/// 未创建托盘时仅更新进程状态；轻量模式重建窗口不会重置此状态。
#[tauri::command]
pub fn set_ui_locale(app: tauri::AppHandle, locale: String) -> Result<(), String> {
    CURRENT_LOCALE.store(normalize_locale(&locale) as u8, Ordering::Release);
    crate::tray::refresh_menu(&app).map_err(|error| error.to_string())
}

#[cfg(windows)]
fn system_locale_name() -> Option<String> {
    use windows_sys::Win32::Globalization::GetUserDefaultLocaleName;

    // Win32 的 LOCALE_NAME_MAX_LENGTH 固定为 85，包含结尾的 NUL。
    let mut buffer = [0_u16; 85];
    // SAFETY: buffer 可写且长度与传入的字符数一致；成功时返回值包含结尾 NUL。
    let length = unsafe { GetUserDefaultLocaleName(buffer.as_mut_ptr(), buffer.len() as i32) };
    if length <= 1 || length as usize > buffer.len() {
        return None;
    }
    Some(String::from_utf16_lossy(&buffer[..length as usize - 1]))
}

#[cfg(not(windows))]
fn system_locale_name() -> Option<String> {
    environment_locale().or_else(preferred_platform_locale)
}

#[cfg(not(windows))]
fn environment_locale() -> Option<String> {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .find_map(|name| {
            std::env::var(name).ok().and_then(|value| {
                let value = value.trim();
                (!value.is_empty() && value != "C" && value != "POSIX").then(|| value.to_string())
            })
        })
}

#[cfg(target_os = "macos")]
fn preferred_platform_locale() -> Option<String> {
    let output = std::process::Command::new("/usr/bin/defaults")
        .args(["read", "-g", "AppleLocale"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let locale = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!locale.is_empty()).then_some(locale)
}

#[cfg(not(any(windows, target_os = "macos")))]
fn preferred_platform_locale() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_supported_and_posix_locales() {
        let cases = [
            ("zh-Hans", Locale::ZhHans),
            ("zh_CN.UTF-8", Locale::ZhHans),
            ("zh", Locale::ZhHans),
            ("ZH-Hant-TW", Locale::ZhHant),
            ("zh_TW", Locale::ZhHant),
            ("zh-HK", Locale::ZhHant),
            ("zh-CHT", Locale::ZhHant),
            ("en-US", Locale::En),
            ("ja_JP.UTF-8", Locale::Ja),
            ("ko-KR", Locale::Ko),
            ("pt-BR", Locale::PtBr),
            ("pt_PT@euro", Locale::PtBr),
        ];
        for (input, expected) in cases {
            assert_eq!(normalize_locale(input), expected, "input: {input}");
        }
    }

    #[test]
    fn unknown_locale_falls_back_to_english() {
        for input in ["", "  ", "C", "POSIX", "fr-FR", "de_DE.UTF-8", "unknown"] {
            assert_eq!(normalize_locale(input), Locale::En, "input: {input}");
        }
    }

    #[test]
    fn every_message_is_complete_in_all_six_languages() {
        for locale in Locale::ALL {
            for key in TextKey::ALL {
                assert!(
                    !text_for(locale, key).trim().is_empty(),
                    "{locale:?} {key:?}"
                );
            }
            assert!(text_for(locale, TextKey::StartupFailureMessage).contains("{error}"));
        }
    }

    #[test]
    fn primary_tray_and_dialog_mappings_are_stable() {
        assert_eq!(
            text_for(Locale::En, TextKey::TrayShowMain),
            "Show Main Window"
        );
        assert_eq!(
            text_for(Locale::ZhHant, TextKey::TrayCheckUpdate),
            "檢查更新"
        );
        assert_eq!(
            text_for(Locale::Ja, TextKey::ExportAccounts),
            "アカウントのエクスポート"
        );
        assert_eq!(text_for(Locale::Ko, TextKey::TrayQuit), "종료");
        assert_eq!(text_for(Locale::PtBr, TextKey::SaveImage), "Salvar imagem");
        assert_eq!(text_for(Locale::En, TextKey::Cancel), "Cancel");
    }

    #[test]
    fn startup_message_preserves_the_original_error_detail() {
        let message =
            startup_failure_message_for(Locale::En, "migration failed: permission denied");
        assert!(
            message
                .starts_with("Agent2API startup was aborted: migration failed: permission denied")
        );
        assert!(message.contains("No existing data was changed or deleted"));

        let chinese = startup_failure_message_for(Locale::ZhHans, "原始错误");
        assert!(chinese.starts_with("Agent2API 启动已中止：原始错误\n\n"));
    }
}
