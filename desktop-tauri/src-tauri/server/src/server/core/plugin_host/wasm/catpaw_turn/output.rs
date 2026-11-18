//! 原工具预约与最终输出观察的封闭配对；框架取帧不是HTTP写完或客户端接收。

use std::sync::{Arc, Mutex, Weak};

use agent2api_provider_protocol as proto;
use serde_json::Value;

use super::{output_view::{ExpectedOutput, OutputObservation, OutputObserver}, registry::Record, tool_evidence::ToolEvidence};
use crate::server::core::plugin_host::wasm::conversation::SourceRef;

#[derive(Clone, Copy, PartialEq, Eq)]
enum OutputState { Pending, Observed, Failed }

trait Original: Send + Sync {
    fn matches(&self, source: &SourceRef, record: &Record, sequence: u64) -> bool;
    fn verify(&self, observation: &OutputObservation) -> Result<(), proto::WireError>;
}

struct OriginalBinding {
    source: SourceRef,
    record: Weak<Record>,
    sequence: u64,
}

impl Original for OriginalBinding {
    fn matches(&self, source: &SourceRef, record: &Record, sequence: u64) -> bool {
        self.source.same_source(source) && self.sequence == sequence
            && std::ptr::eq(self.record.as_ptr(), record)
    }

    fn verify(&self, observation: &OutputObservation) -> Result<(), proto::WireError> {
        let record = self.record.upgrade().ok_or_else(unconfirmed)?;
        if !record.output_reservation_matches(&self.source, self.sequence)
            || !observation.matches_identity(&record.ids.response,
                record.prepared.constraints["model_name"].as_str().ok_or_else(unconfirmed)?, record.ids.created) {
            return Err(unconfirmed());
        }
        Ok(())
    }
}

struct Binding {
    original: Box<dyn Original>,
    expected: ExpectedOutput,
}

impl Binding {
    fn verify(&self, observation: &OutputObservation) -> Result<(), proto::WireError> {
        self.original.verify(observation)?;
        self.expected.verify(observation)
    }
}

struct State {
    guard_issued: bool,
    selected: bool,
    attempt_closed: bool,
    binding: Option<Binding>,
    observation: Option<OutputObservation>,
    output: OutputState,
    body_ended: bool,
}

pub(in crate::server) struct OutputSlot { state: Mutex<State> }

impl OutputSlot {
    pub(in crate::server::core::plugin_host) fn new() -> Arc<Self> {
        Arc::new(Self { state: Mutex::new(State {
            guard_issued: false, selected: false, attempt_closed: false,
            binding: None, observation: None,
            output: OutputState::Pending, body_ended: false,
        }) })
    }

    pub(in crate::server::core::plugin_host) fn select(&self) -> Result<(), proto::WireError> {
        let mut state = self.state.try_lock().map_err(|_| unconfirmed())?;
        if state.selected || state.guard_issued || state.output == OutputState::Failed {
            return Err(unconfirmed());
        }
        state.selected = true;
        Ok(())
    }

    pub(in crate::server::core::plugin_host) fn close_attempt(&self) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.attempt_closed = true;
    }

    pub(in crate::server) fn replaceable(&self) -> bool {
        self.state.try_lock().is_ok_and(|state| state.attempt_closed && !state.selected
            && !state.guard_issued && state.binding.is_none())
    }

    pub(in crate::server) fn take(self: &Arc<Self>) -> Result<Option<OutputGuard>, proto::WireError> {
        let mut state = self.state.try_lock().map_err(|_| unconfirmed())?;
        if !state.selected { return Ok(None); }
        if state.guard_issued || state.output == OutputState::Failed { return Err(unconfirmed()); }
        state.guard_issued = true;
        Ok(Some(OutputGuard { slot: self.clone(), observer: Some(OutputObserver::new()), prepared: false, finished: false }))
    }

    pub(super) fn bind(
        &self, source: &SourceRef, record: &Arc<Record>, sequence: u64, evidence: &ToolEvidence,
    ) -> Result<(), proto::WireError> {
        self.bind_expected(source, record, sequence, ExpectedOutput::from_evidence(evidence)?)
    }

    pub(super) fn bind_message(&self, source: &SourceRef, record: &Arc<Record>, sequence: u64, message: &Value)
        -> Result<(), proto::WireError> {
        self.bind_expected(source, record, sequence, ExpectedOutput::from_message(message)?)
    }

    fn bind_expected(&self, source: &SourceRef, record: &Arc<Record>, sequence: u64, expected: ExpectedOutput)
        -> Result<(), proto::WireError> {
        let result = (|| {
            if !source.belongs_to_output(self) || !record.output_reservation_matches(source, sequence) {
                return Err(unconfirmed());
            }
            let binding = Binding {
                original: Box::new(OriginalBinding { source: source.clone(), record: Arc::downgrade(record), sequence }),
                expected,
            };
            let mut state = self.state.try_lock().map_err(|_| unconfirmed())?;
            if state.binding.is_some() || state.output == OutputState::Failed { return Err(unconfirmed()); }
            if let Some(observation) = &state.observation { binding.verify(observation)?; }
            state.output = if state.observation.is_some() { OutputState::Observed } else { OutputState::Pending };
            state.binding = Some(binding);
            Ok(())
        })();
        if result.is_err() { self.fail(); }
        result
    }

    fn observe(&self, observation: OutputObservation) -> Result<(), proto::WireError> {
        let result = (|| {
            let mut state = self.state.try_lock().map_err(|_| unconfirmed())?;
            if !state.guard_issued || state.observation.is_some() || state.output == OutputState::Failed {
                return Err(unconfirmed());
            }
            if let Some(binding) = &state.binding { binding.verify(&observation)?; }
            state.output = if state.binding.is_some() { OutputState::Observed } else { OutputState::Pending };
            state.observation = Some(observation);
            Ok(())
        })();
        if result.is_err() { self.fail(); }
        result
    }

    fn prepared(&self) -> bool {
        self.state.try_lock().is_ok_and(|state| state.output == OutputState::Observed
            && state.binding.is_some() && state.observation.is_some())
    }

    fn finish_body(&self) -> Result<(), proto::WireError> {
        let mut state = self.state.try_lock().map_err(|_| unconfirmed())?;
        if state.body_ended || state.observation.is_none() || state.binding.is_none()
            || state.output != OutputState::Observed {
            return Err(unconfirmed());
        }
        state.body_ended = true;
        Ok(())
    }

    fn fail(&self) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.output = OutputState::Failed;
    }

    pub(super) fn failed(&self) -> bool {
        self.state.try_lock().map(|state| state.output == OutputState::Failed).unwrap_or(true)
    }

    pub(super) fn observed(&self, source: &SourceRef, record: &Record, sequence: u64) -> bool {
        self.state.try_lock().is_ok_and(|state| {
            state.output == OutputState::Observed && state.body_ended
                && state.binding.as_ref().is_some_and(|binding| binding.original.matches(source, record, sequence))
        })
    }
}

pub(in crate::server) struct OutputGuard {
    slot: Arc<OutputSlot>,
    observer: Option<OutputObserver>,
    prepared: bool,
    finished: bool,
}

impl OutputGuard {
    pub(in crate::server) fn push(&mut self, bytes: &[u8]) -> Result<(), proto::WireError> {
        let result = self.observer.as_mut().ok_or_else(unconfirmed)?.push(bytes);
        if result.is_err() { self.slot.fail(); }
        result
    }

    pub(in crate::server) fn observe_json(&mut self, bytes: &[u8]) -> Result<(), proto::WireError> {
        let result = (|| {
            let text = std::str::from_utf8(bytes).map_err(|_| unconfirmed())?;
            let body: Value = super::json_text::parse(text, 2 * 1024 * 1024)?;
            self.observer.as_mut().ok_or_else(unconfirmed)?.observe_body(&body)
        })();
        if result.is_err() { self.slot.fail(); }
        result
    }

    pub(in crate::server) fn terminal_seen(&self) -> bool {
        self.observer.as_ref().is_some_and(OutputObserver::terminal_seen)
    }

    pub(in crate::server) fn prepare_chat(&mut self) -> Result<Value, proto::WireError> {
        let result = (|| {
            if self.prepared { return Err(unconfirmed()); }
            let observation = self.observer.take().ok_or_else(unconfirmed)?.finish()?;
            let chat = observation.chat_body();
            self.slot.observe(observation)?;
            if !self.slot.prepared() { return Err(unconfirmed()); }
            self.prepared = true;
            Ok(chat)
        })();
        if result.is_err() { self.slot.fail(); }
        result
    }

    pub(in crate::server) fn verify_converted(&self, body: &Value, request: &Value, model: &str, responses: bool)
        -> Result<(), proto::WireError> {
        let result = (|| {
            let state = self.slot.state.try_lock().map_err(|_| unconfirmed())?;
            if !self.prepared || state.output != OutputState::Observed { return Err(unconfirmed()); }
            let observation = state.observation.as_ref().ok_or_else(unconfirmed)?;
            if responses { super::output_view::verify_responses(observation, body, request, model) }
            else { super::output_view::verify_anthropic(observation, body, model) }
        })();
        if result.is_err() { self.slot.fail(); }
        result
    }

    pub(in crate::server) fn prepare(&mut self) -> Result<(), proto::WireError> {
        let result = (|| {
            if self.prepared { return Err(unconfirmed()); }
            let observation = self.observer.take().ok_or_else(unconfirmed)?.finish()?;
            self.slot.observe(observation)?;
            if !self.slot.prepared() { return Err(unconfirmed()); }
            self.prepared = true;
            Ok(())
        })();
        if result.is_err() { self.slot.fail(); }
        result
    }

    pub(in crate::server) fn finish(mut self) -> Result<(), proto::WireError> {
        let result = (|| {
            if !self.prepared { self.prepare()?; }
            self.slot.finish_body()
        })();
        if result.is_err() { self.slot.fail(); }
        self.finished = true;
        result
    }

    pub(in crate::server) fn fail(&mut self) {
        self.slot.fail();
        self.finished = true;
    }
}

impl Drop for OutputGuard {
    fn drop(&mut self) { if !self.finished { self.slot.fail(); } }
}

pub(in crate::server) fn unconfirmed() -> proto::WireError {
    proto::WireError::with_code(
        "CatPaw 最终工具输出未完整核对，未把框架取帧当作HTTP写完或客户端接收，也未开放续接",
        "catpaw_output_unconfirmed",
    )
}
