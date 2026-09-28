//! Deterministic host used by both native and WebAssembly conformance runners.
use crate::{
    error::AzaError,
    interpreter::{Host, InputRequest, Interpreter},
    values::{format_value, Arr},
};
use serde_json::{json, Value};
use std::{
    cell::RefCell,
    collections::{BTreeMap, VecDeque},
    rc::Rc,
};

#[derive(Default)]
struct State {
    inputs: VecDeque<String>,
    available_inputs: Vec<String>,
    files: BTreeMap<String, String>,
    outside: BTreeMap<String, Value>,
    requests: Vec<InputRequest>,
    slept: f64,
    sleeps: usize,
    stop_immediately: bool,
    stop_after_sleeps: Option<usize>,
}
struct ScriptHost(Rc<RefCell<State>>);
impl Host for ScriptHost {
    fn supports_input(&self, kind: &str) -> bool {
        self.0.borrow().available_inputs.iter().any(|k| k == kind)
    }
    fn input(&mut self, request: &InputRequest) -> Result<String, AzaError> {
        let mut s = self.0.borrow_mut();
        s.requests.push(request.clone());
        s.inputs
            .pop_front()
            .ok_or_else(|| AzaError::new("入力を受け取れません"))
    }
    fn sleep(&mut self, seconds: f64) -> Result<(), AzaError> {
        let mut s = self.0.borrow_mut();
        s.slept += seconds;
        s.sleeps += 1;
        Ok(())
    }
    fn outside(&mut self, name: &str) -> Result<Arr, AzaError> {
        self.0
            .borrow()
            .outside
            .get(name)
            .ok_or_else(|| AzaError::new("外の値がありません"))
            .and_then(Arr::from_json)
    }
    fn read_file(&mut self, name: &str) -> Result<String, AzaError> {
        crate::interpreter::validate_file_name(name)?;
        self.0
            .borrow()
            .files
            .get(name)
            .cloned()
            .ok_or_else(|| AzaError::new("ファイルがありません"))
    }
    fn write_file(&mut self, name: &str, text: &str) -> Result<(), AzaError> {
        crate::interpreter::validate_file_name(name)?;
        self.0.borrow_mut().files.insert(name.into(), text.into());
        Ok(())
    }
    fn stopped(&self) -> bool {
        let s = self.0.borrow();
        s.stop_immediately || s.stop_after_sleeps.is_some_and(|n| s.sleeps >= n)
    }
}

pub fn run_case(case: &Value) -> Value {
    let mode = case["mode"].as_str().unwrap_or("run");
    let program = match crate::parser::parse(case["source"].as_str().unwrap_or("")) {
        Ok(p) => p,
        Err(mut e) => {
            e.kind = "syntax".into();
            return json!({"error":e});
        }
    };
    match mode {
        "parse" => return json!({"ok":true}),
        "readings" => return json!({"lines":crate::readings::read_program(&program)}),
        "expand" => return json!({"lines":crate::expand::expand_program(&program)}),
        _ => (),
    }
    let h = &case["host"];
    let state = Rc::new(RefCell::new(State {
        inputs: serde_json::from_value(h["inputs"].clone()).unwrap_or_default(),
        available_inputs: serde_json::from_value(h["available_inputs"].clone()).unwrap_or_default(),
        files: serde_json::from_value(h["files"].clone()).unwrap_or_default(),
        outside: serde_json::from_value(h["outside"].clone()).unwrap_or_default(),
        stop_immediately: h["stop_immediately"].as_bool().unwrap_or(false),
        stop_after_sleeps: h["stop_after_sleeps"].as_u64().map(|n| n as usize),
        ..State::default()
    }));
    crate::primitives::configure_random_big(
        h["seed"].as_u64().unwrap_or(12345),
        h["random_values"]
            .as_array()
            .map(|xs| {
                xs.iter()
                    .map(|v| {
                        v.as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| v.to_string())
                            .parse()
                            .expect("integer random fixture")
                    })
                    .collect()
            })
            .unwrap_or_default(),
        serde_json::from_value(h["random_deals"].clone()).unwrap_or_default(),
        serde_json::from_value(h["random_shuffles"].clone()).unwrap_or_default(),
    );
    let mut interpreter = Interpreter::new(Box::new(ScriptHost(state.clone())));
    let error = interpreter.run(&program).err();
    let s = state.borrow();
    json!({
        "results":interpreter.results.iter().map(|(line,a)| json!({"line":line,"value":a.to_json(),"text":format_value(a)})).collect::<Vec<_>>(),
        "events":interpreter.events.iter().map(|e|json!({"kind":e.kind,"line":e.line,"value":e.value.to_json(),"text":e.text})).collect::<Vec<_>>(),
        "requests":s.requests,"ended":interpreter.ended,"files":s.files,"slept":s.slept,"error":error
    })
}

/// JSON paths make a discrepancy reproducible without depending on either engine's display.
pub fn difference(expected: &Value, actual: &Value, path: &str) -> Option<String> {
    if expected.get("type").and_then(Value::as_str) == Some("float")
        && actual.get("type").and_then(Value::as_str) == Some("float")
    {
        let number = |v: &Value| {
            v.as_f64()
                .or_else(|| v.as_str().and_then(|s| s.parse::<f64>().ok()))
        };
        if let (Some(a), Some(b)) = (number(&expected["value"]), number(&actual["value"])) {
            if (a.is_nan() && b.is_nan()) || a == b || (a - b).abs() <= 1e-12 + 1e-12 * a.abs() {
                return None;
            }
        }
    }
    match (expected, actual) {
        (Value::Object(a), Value::Object(b)) => {
            for (k, v) in a {
                if let Some(d) =
                    difference(v, b.get(k).unwrap_or(&Value::Null), &format!("{path}.{k}"))
                {
                    return Some(d);
                }
            }
            None
        }
        (Value::Array(a), Value::Array(b)) => {
            if a.len() != b.len() {
                return Some(format!("{path}: length {} != {}", a.len(), b.len()));
            }
            a.iter()
                .zip(b)
                .enumerate()
                .find_map(|(i, (x, y))| difference(x, y, &format!("{path}[{i}]")))
        }
        (Value::Number(a), Value::Number(b))
            if a.as_f64()
                .zip(b.as_f64())
                .is_some_and(|(a, b)| (a - b).abs() <= 1e-12 + 1e-12 * a.abs()) =>
        {
            None
        }
        _ if expected == actual => None,
        _ => Some(format!("{path}: expected {expected}, got {actual}")),
    }
}
