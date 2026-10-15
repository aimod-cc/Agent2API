//! 「长度截断自动续写」的开关与轮数上限：`GET/PUT /api/auto-continue`。
//!
//! ── 这个端点做什么 ──────────────────────────────────────────
//! 上游的 turn 会**中途收尾**：输出预算用尽（`finish_reason: length` 一族）、连接被
//! 掐断、或者整轮烧在思考通道里、正文只吐一两个词。客户端拿到的是半截答案，
//! 而它无法区分「模型就说这么多」与「被腰斩」。开启后转发层把**已经写出的那半截**
//! 当作 assistant 回合回放给上游、追一句「请继续输出」再打一次，新内容接在
//! **同一条**客户端响应后面。六条判据、两道停机条件（轮数上限 + 相似度闸）与
//! 代价口径都在 [`crate::server::core::upstream::continuation`]，这里只管两件事：
//! 开不开（`autoContinue`）、最多问几轮（`autoContinueMaxRounds`，默认 3、合法 1~8）。
//!
//! ── 为什么一个端点装两项，而不是照 keepalive 那样一项一个开关 ──
//! 因为这两项**必须一起读才有意义**：续写链的两道停机里，轮数上限是那道**硬保险**
//! （另一道是相似度闸，判的是「续出来的是不是上一段的复读」，阈值是软的），
//! 一次请求最多打上游几回只由这两个数合起来决定。拆成两个端点则会出现
//! 「开关开了、上限还没设」的中间态 —— 那是同一次设置动作的两半，所以合成一条
//! PUT（与 `/api/routing` 那条「一个开关 + 几个有界整数」同形）。
//!
//! ── 为什么默认关、又为什么要能在线切 ───────────────────────
//! 代价是真金白银的：续写的每一轮都带着前几轮的全部历史，一轮的成本**不覆盖**
//! 前一轮而是求和，而收益只落在「这一轮确实被腰斩」的那一类请求上。花额度 +
//! 改「客户端看到什么」这种可观测行为，两条都够格让默认值取关（见
//! `config::KEY_AUTO_CONTINUE` 的取舍说明）。留成在线端点，是为了**能在真流量上
//! 验证并随时撤回**：开一下、看几条请求、不对就关掉，不需要重新构建镜像。
//! 转发层逐请求读内存快照，改完下一个请求立即生效，不重启进程。
//!
//! ── 为什么两项能分开改（partial update）─────────────────────
//! 调用场景是分开的：先用默认 3 轮验一次效果，之后只想把预算收到 1 轮 ——
//! 那一次 PUT 不该顺手把开关写回它已有的值（一次多余的落盘与一条多余的日志），
//! 更不该要求调用方先 GET 才能安全地 PUT。所以**出现哪一项就改哪一项**，
//! 缺省项保持原值；同时校验全部通过才落盘（与 `put_retry` / `put_retention`
//! 同一顺序），避免出现「开关改了、上限没改」的半套设置。
//!
//! ── 与 /api/keepalive-strip、/api/sanitize 的关系 ────────────
//! 形状刻意保持一致（GET 读 / PUT 写 / 响应体就是新状态），差别只在写侧项目数：
//! 那两条各一个布尔值，本条是一个布尔值 + 一个有界整数（于是多了范围校验）。
//! 三者都是「改客户端看到的字节 / 改转发链的行为」且默认关，因此共用同一套
//! 读写模式与同一条事件日志口径（`[Config]`，开关两个方向都记）。

use axum::body::Bytes;
use axum::extract::State;
use axum::response::Response;
use serde_json::{json, Map, Value};

use crate::server::config::{
    self, AutoContinuePatch, KEY_AUTO_CONTINUE, KEY_AUTO_CONTINUE_MAX_ROUNDS,
    AUTO_CONTINUE_MAX_ROUNDS, AUTO_CONTINUE_MIN_ROUNDS,
};
use crate::server::config::parse::int_in_range;
use crate::server::errors;
use crate::server::http::{ok_json, parse_body};
use crate::server::logging;
use crate::server::ServerState;

/// GET /api/auto-continue —— 当前开关与轮数上限
pub async fn get_auto_continue(State(_state): State<ServerState>) -> Response {
    ok_json(state_json())
}

/// PUT /api/auto-continue —— body `{autoContinue?: true|false, autoContinueMaxRounds?: 1~8}`
///
/// 两项各自独立（缺省项不动），全部校验通过才落盘。写配置后下一个请求立即生效
/// （转发层逐请求读快照，不重启进程）。开与关都记一条事件日志：开的那一刻起，
/// 一次客户端请求可能对应多次上游往返、多花一份额度，而关的那一刻起又变回去 ——
/// 排查「这次请求怎么打了四回上游」时，日志里必须查得到这条线。
pub async fn put_auto_continue(State(_state): State<ServerState>, body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(value) => value,
        Err(error) => return errors::management_error(400, error.message),
    };
    let Some(object) = payload.as_object() else {
        return errors::management_error(400, "请求体必须是 JSON 对象");
    };
    let patch = match patch_from(object) {
        Ok(patch) => patch,
        Err(message) => return errors::management_error(400, message),
    };
    if patch.enabled.is_none() && patch.max_rounds.is_none() {
        // 两项都没给：不写盘、不记日志（一条「设置已更新」的日志底下什么都没有，
        // 会让排障的人以为发生过一次改动），直接把当前状态原样回给调用方
        return ok_json(state_json());
    }
    if !config::set_auto_continue_patch(patch) {
        logging::log("[Config]", "⚠️  自动续写设置写入配置失败，本次运行内仍生效");
    }
    logging::log("[Config]", &change_note(&patch));
    ok_json(state_json())
}

/// 请求体 → 部分更新入参（`Err` 的文本就是 400 的文案）。
///
/// ── 为什么抽成纯函数 ────────────────────────────────────────
/// 判据要能被单测钉住：`ServerState::bootstrap` 会开真库、写 `~/.agent2api`，
/// 用例驱动不了 handler 本身，而这几条判定恰恰是「写坏了静默伤到每一条请求」
/// 的那一类（与 `api::cline_headers` 的 `validated_overrides` 同一手法）。
///
/// ── 为什么不认 `null` ───────────────────────────────────────
/// 与 [`KEY_AUTO_CONTINUE`]、[`KEY_AUTO_CONTINUE_MAX_ROUNDS`] 的读侧口径**一致**：
/// 缺失 = 用当前值（不改），出现了就必须是合法值。`null` 出现在这里只能是值打错
/// —— 这条 PUT 没有「删掉这一项」的语义（setter 只 insert，落库侧的删除只跟着
/// 底稿走），所以 `null` 什么也表达不了：既不是「关掉」（那是 `false`），也不是
/// 「回到未配」（那要改库删行）。像 `put_retry` 那样把它当成「这一项不改」放过，
/// 结果是调用方收到 200 而实际什么都没变。
fn patch_from(object: &Map<String, Value>) -> Result<AutoContinuePatch, String> {
    let mut patch = AutoContinuePatch::default();
    if let Some(value) = object.get(KEY_AUTO_CONTINUE) {
        let Some(enabled) = value.as_bool() else {
            return Err(format!("{KEY_AUTO_CONTINUE} 必须是 true 或 false"));
        };
        patch.enabled = Some(enabled);
    }
    if let Some(value) = object.get(KEY_AUTO_CONTINUE_MAX_ROUNDS) {
        // 判据与读侧共用 `int_in_range`（含「接受 3.0 这种整值浮点」这一条）：
        // 范围与「什么算一个整数」两侧各写一份，迟早出现接口拒绝而手改库接受
        let Some(rounds) = int_in_range(
            value,
            AUTO_CONTINUE_MIN_ROUNDS,
            AUTO_CONTINUE_MAX_ROUNDS,
        ) else {
            return Err(format!(
                "{KEY_AUTO_CONTINUE_MAX_ROUNDS} 必须是 {AUTO_CONTINUE_MIN_ROUNDS}~{AUTO_CONTINUE_MAX_ROUNDS} 的整数"
            ));
        };
        patch.max_rounds = Some(rounds);
    }
    Ok(patch)
}

/// 这次改动的一句话记录（进事件日志）。
///
/// 三种形态分开写：只改开关 / 只改上限 / 两项一起改。日志里要能看出**这次 PUT
/// 动了哪一项**，否则「谁把上限从 3 改到 8 的」这类问题只能靠翻请求体回答。
fn change_note(patch: &AutoContinuePatch) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(enabled) = patch.enabled {
        parts.push(if enabled {
            "自动续写已开启（一次被腰斩的回答会由网关自己问完）".to_string()
        } else {
            "自动续写已关闭：半截回答原样下发（与接入前逐字节一致）".to_string()
        });
    }
    if let Some(rounds) = patch.max_rounds {
        parts.push(format!("续写轮数上限已设为 {rounds}"));
    }
    parts.join(" / ")
}

/// 开关与上限（GET 与 PUT 共用，前端直接用响应刷新界面）
fn state_json() -> Value {
    json!({
        KEY_AUTO_CONTINUE: config::auto_continue(),
        KEY_AUTO_CONTINUE_MAX_ROUNDS: config::auto_continue_max_rounds(),
    })
}

#[cfg(test)]
mod tests {
    //! PUT 的两条判据：类型与范围。
    //!
    //! ── 为什么钉在这里而不是响应状态码上 ────────────────────
    //! handler 需要的 `ServerState` 只能由 `ServerState::bootstrap` 构造，而它会
    //! 开真库、写 `~/.agent2api` —— 用例驱动不了它。于是判据抽成 [`patch_from`]
    //! （返回 `Err(文案)`），handler 那侧只有一句
    //! `Err(message) => errors::management_error(400, message)`：文案错不了，
    //! 而 400 是那句话本身，不是可以再写歪的逻辑。与 `api::cline_headers`、
    //! `api::codearts_welfare` 的同一取舍。
    //!
    //! ── 每个用例钉的是哪种「写歪」───────────────────────────
    //! 这三条各自对应一个**看起来合理**的实现，而不只是补覆盖率：
    //! 认 `null`、用 `as_i64`（于是 3.0 被拒）、缺项时填默认值。

    use super::*;

    /// 一份只含给定几项的请求体。
    fn body(pairs: &[(&str, Value)]) -> Map<String, Value> {
        pairs.iter().map(|(key, value)| (key.to_string(), value.clone())).collect()
    }

    /// 布尔项只认字面 `true` / `false`；`null` 也算类型不符。
    ///
    /// 钉住的写歪：把 `null` 当成「没给」（`put_retry` 对 null 是这么处理的，
    /// 那里合理 —— 表单整份回传会有未填项；这里两个键都没有「清空」语义），
    /// 于是调用方收到 200 而开关一动没动。
    #[test]
    fn the_switch_accepts_only_a_literal_boolean() {
        let only_false = patch_from(&body(&[(KEY_AUTO_CONTINUE, json!(false))]))
            .expect("字面 false 是合法值");
        assert_eq!(only_false.enabled, Some(false), "关掉必须能被表达出来");

        for bad in [json!("true"), json!(1), json!(null), json!({})] {
            let error = match patch_from(&body(&[(KEY_AUTO_CONTINUE, bad.clone())])) {
                Ok(patch) => panic!("{bad} 不该被接受，却拿到了 {patch:?}"),
                Err(error) => error,
            };
            assert!(
                error.contains("必须是 true 或 false"),
                "文案要与 /api/keepalive-strip 同形，拿到的是: {error}"
            );
        }
    }

    /// 轮数上限的边界：1 与 8 都收，0 / 9 / 越界远的都拒；`3.0` 这种整值浮点**必须收**。
    ///
    /// 钉住的写歪：判据改用 `value.as_i64()`（`routing_api` 的 `int_field` 就是
    /// 这么写的）—— 那样读侧接受 3.0、写侧 400，同一份配置两条口径
    /// （`parse::int_in_range` 的存在理由就是把它们钉成一条）。
    #[test]
    fn the_round_cap_is_validated_against_the_same_range_as_the_read_side() {
        for (accepted, expected) in [(json!(1), 1), (json!(8), 8), (json!(3.0), 3)] {
            let patch = match patch_from(&body(&[(KEY_AUTO_CONTINUE_MAX_ROUNDS, accepted.clone())]))
            {
                Ok(patch) => patch,
                Err(error) => panic!("{accepted} 应当被接受，却报错: {error}"),
            };
            assert_eq!(
                patch.max_rounds,
                Some(expected),
                "整值浮点要按它表示的那个整数落盘，不是回落默认值"
            );
        }
        for rejected in [json!(0), json!(9), json!(-1), json!("3"), json!(null), json!(2.5)] {
            let error = match patch_from(&body(&[(
                KEY_AUTO_CONTINUE_MAX_ROUNDS,
                rejected.clone(),
            )])) {
                Ok(patch) => panic!("{rejected} 越界或类型不符，不该被接受，却拿到了 {patch:?}"),
                Err(error) => error,
            };
            assert_eq!(
                error,
                format!(
                    "{KEY_AUTO_CONTINUE_MAX_ROUNDS} 必须是 {AUTO_CONTINUE_MIN_ROUNDS}~{AUTO_CONTINUE_MAX_ROUNDS} 的整数"
                ),
                "文案是接口契约（面板按它提示），且必须带上真正的边界",
            );
        }
    }

    /// 两项独立：只出现一项时另一项是 `None`（= 不动），都不出现时两项都是 `None`。
    ///
    /// 钉住的写歪：缺省项填成默认值 —— 那会让「只想把预算收到 1 轮」这一次 PUT
    /// 顺手把开关写回关（用户刚打开的功能被自己下一次的调整抹掉），
    /// 且 `set_auto_continue_patch` 那边根本分不出「没给」与「给的是默认值」。
    #[test]
    fn an_absent_key_stays_absent_rather_than_becoming_the_default() {
        let only_switch = patch_from(&body(&[(KEY_AUTO_CONTINUE, json!(true))])).expect("合法");
        assert_eq!((only_switch.enabled, only_switch.max_rounds), (Some(true), None));

        let only_rounds =
            patch_from(&body(&[(KEY_AUTO_CONTINUE_MAX_ROUNDS, json!(1))])).expect("合法");
        assert_eq!((only_rounds.enabled, only_rounds.max_rounds), (None, Some(1)));

        let both = patch_from(&body(&[
            (KEY_AUTO_CONTINUE, json!(true)),
            (KEY_AUTO_CONTINUE_MAX_ROUNDS, json!(8)),
        ]))
        .expect("合法");
        assert_eq!((both.enabled, both.max_rounds), (Some(true), Some(8)));

        // 空对象 = 什么都没给：不报错（一次 PUT 只改一项是常态，全给也是它），
        // 但两项都必须是 None，否则 handler 会落一次没有任何变化的盘
        let nothing = patch_from(&body(&[])).expect("空对象不该报错");
        assert_eq!(nothing.enabled, None);
        assert_eq!(nothing.max_rounds, None);
    }
}
