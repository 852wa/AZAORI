use crate::{
    error::AzaError,
    interpreter::{Event, Host, InputRequest, Interpreter},
    values::Arr,
};
use serde_json::{json, Value};
use wasm_bindgen::prelude::*;

struct JsHost(js_sys::Function);
impl JsHost {
    fn call(&self, method: &str, payload: Value) -> Result<Value, AzaError> {
        let answer = self
            .0
            .call2(
                &JsValue::NULL,
                &JsValue::from_str(method),
                &JsValue::from_str(&payload.to_string()),
            )
            .map_err(|e| {
                AzaError::new(e.as_string().unwrap_or_else(|| format!("{e:?}"))).with_kind("host")
            })?;
        let value: Value =
            serde_json::from_str(&answer.as_string().unwrap_or_else(|| "null".into()))
                .map_err(|e| AzaError::new(e.to_string()).with_kind("host"))?;
        if let Some(message) = value.get("host_error").and_then(Value::as_str) {
            return Err(AzaError::new(message).with_kind("host"));
        }
        Ok(value)
    }
}
impl Host for JsHost {
    fn input(&mut self, request: &InputRequest) -> Result<String, AzaError> {
        Ok(self
            .call("input", json!(request))?
            .as_str()
            .unwrap_or("")
            .into())
    }
    fn sleep(&mut self, seconds: f64) -> Result<(), AzaError> {
        self.call("sleep", json!(seconds))?;
        Ok(())
    }
    fn outside(&mut self, name: &str) -> Result<Arr, AzaError> {
        Arr::from_json(&self.call("outside", json!(name))?)
    }
    fn read_file(&mut self, name: &str) -> Result<String, AzaError> {
        Ok(self
            .call("read_file", json!(name))?
            .as_str()
            .unwrap_or("")
            .into())
    }
    fn write_file(&mut self, name: &str, text: &str) -> Result<(), AzaError> {
        self.call("write_file", json!({"name":name,"text":text}))?;
        Ok(())
    }
    fn event(&mut self, event: &Event) {
        let _ = self.call("event", json!(event));
    }
}

thread_local! {
    static LAST_PANIC: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

/// 読み込んだときに1回だけ動く。panic の中身を覚えておく（ブラウザには「unreachable」としか出ないため）
#[wasm_bindgen(start)]
pub fn start() {
    std::panic::set_hook(Box::new(|info| {
        LAST_PANIC.with(|last| *last.borrow_mut() = info.to_string());
    }));
}

/// 最後に起きた panic の中身（なければ空）
#[wasm_bindgen]
pub fn last_panic() -> String {
    LAST_PANIC.with(|last| last.borrow().clone())
}

#[wasm_bindgen]
pub fn analyze(source: &str) -> String {
    let result = (|| -> Result<Value, AzaError> {
        let tokens = crate::lexer::tokenize(source)?;
        let (program, elisions) = crate::parser::parse_with_elisions(source)?;
        Ok(json!({"tokens":tokens,"tree":program,"elisions":elisions,
            "analysis":crate::analysis::analyze_program(&program),
            "readings":crate::readings::read_program(&program).iter().map(|(line,text)|json!({"line":line,"text":text})).collect::<Vec<_>>(),
            "marked_readings":crate::readings::read_program_marked(&program).iter().map(|(line,text)|json!({"line":line,"text":text})).collect::<Vec<_>>(),
            "expanded":crate::expand::expand_program(&program).iter().map(|(line,text)|json!({"line":line,"text":text})).collect::<Vec<_>>(),"error":null}))
    })();
    result
        .unwrap_or_else(|e| json!({"tokens":[],"tree":null,"elisions":[],"analysis":[],"readings":[],"expanded":[],"error":e}))
        .to_string()
}

#[wasm_bindgen]
pub fn execute(source: &str, host: js_sys::Function) -> String {
    crate::primitives::set_random_seed(js_sys::Date::now() as u64);
    let mut runner = Interpreter::new(Box::new(JsHost(host)));
    runner.trace = Some(Default::default()); // 「中身」タブ用に、これ・それ などを記録する
    let error = match crate::parser::parse(source) {
        Ok(program) => runner.run(&program).err(),
        Err(error) => Some(error),
    };
    let trace = runner.finish_trace();
    let variables = runner.variables();
    json!({"status":if error.is_some(){"error"}else{"done"},"error":error,"ended":runner.ended,"trace":trace,"variables":variables,
        "note":if runner.pauses>100 {Some(format!("「停」の表示は最初の100回までです（全部で{}回）",runner.pauses))}else{None}}).to_string()
}

#[wasm_bindgen]
pub fn conformance_case(case: &str) -> String {
    match serde_json::from_str(case) {
        Ok(case) => crate::conformance::run_case(&case).to_string(),
        Err(error) => json!({"error":{"message":error.to_string()}}).to_string(),
    }
}
