//! Heap-resident continuations keep AZAORI recursion off the host/wasm call stack.
use super::*;
use crate::ast::fn_label;
use std::rc::Rc;

type Action = Box<dyn FnOnce(&mut Machine<'_>) -> Result<(), Signal>>;
type Guard = Box<dyn FnOnce(&mut Machine<'_>)>;

enum Task {
    Action(Action),
    Guard(Guard),
}

struct Machine<'a> {
    interpreter: &'a mut Interpreter,
    tasks: Vec<Task>,
    value: Option<Arr>,
    signal: Option<Signal>,
    root_execution: bool,
}

/// 操作の呼び出し（中身・左右の値・名前・あれ・定義の行）
pub(super) struct Call {
    pub body: Node,
    pub a: Arr,
    pub b: Option<Arr>,
    pub name: String,
    pub are: Option<Arr>,
    pub line: Option<usize>,
}

pub(super) fn run_body(interpreter: &mut Interpreter, call: Call, named: bool) -> EvalResult {
    if named {
        interpreter.locals.push(HashMap::new());
    }
    let mut machine = Machine {
        interpreter,
        tasks: Vec::new(),
        value: None,
        signal: None,
        root_execution: false,
    };
    machine.enter_body(call, false);
    let result = run_machine(machine);
    if named {
        interpreter.locals.pop();
        if let Err(Signal::Error(error)) = &result {
            let mut error = error.clone();
            error.col = None;
            error.end = None;
            return Err(error.into());
        }
    }
    result
}

pub(super) fn run_steps(interpreter: &mut Interpreter, steps: &[Node], subject: Arr) -> EvalResult {
    let mut machine = Machine {
        interpreter,
        tasks: Vec::new(),
        value: None,
        signal: None,
        root_execution: true,
    };
    machine.start_steps(steps.to_vec(), subject);
    run_machine(machine)
}

fn run_machine(mut machine: Machine<'_>) -> EvalResult {
    while let Some(task) = machine.tasks.pop() {
        match task {
            Task::Guard(guard) => guard(&mut machine),
            Task::Action(action) if machine.signal.is_none() => {
                let result = machine
                    .interpreter
                    .check_stop()
                    .and_then(|_| action(&mut machine));
                if let Err(signal) = result {
                    machine.signal = Some(signal);
                }
            }
            Task::Action(_) => (),
        }
    }
    match machine.signal {
        Some(signal) => Err(signal),
        None => machine
            .value
            .ok_or_else(|| AzaError::new("操作の結果がありません").into()),
    }
}

fn eval_sync(interpreter: &mut Interpreter, node: Node) -> EvalResult {
    let mut machine = Machine {
        interpreter,
        tasks: Vec::new(),
        value: None,
        signal: None,
        root_execution: false,
    };
    machine.eval(node);
    run_machine(machine)
}

fn phrase_sync(interpreter: &mut Interpreter, node: Node, subject: Arr) -> EvalResult {
    let mut machine = Machine {
        interpreter,
        tasks: Vec::new(),
        value: None,
        signal: None,
        root_execution: true,
    };
    machine.phrase(node, subject);
    run_machine(machine)
}

fn loop_sync(
    interpreter: &mut Interpreter,
    count: Option<Node>,
    body: Node,
    until: Option<Node>,
    mut acc: Arr,
) -> EvalResult {
    let count = count
        .map(|node| -> Result<usize, Signal> {
            let value = eval_sync(interpreter, node)?;
            let value = as_int(&value, "回")?;
            if value < BigInt::from(0) {
                return Err(AzaError::new("「回」の回数にマイナスの数は使えません").into());
            }
            Ok(value.to_usize().unwrap_or(usize::MAX))
        })
        .transpose()?;
    let mut done = 0;
    loop {
        interpreter.check_stop()?;
        if let Some(until) = &until {
            if truth(&phrase_sync(interpreter, until.clone(), acc.clone())?, "止")? {
                break;
            }
        }
        if count.is_some_and(|count| done >= count) {
            break;
        }
        done += 1;
        if done > MAX_LOOP {
            return Err(AzaError::new(format!(
                "「回」が{MAX_LOOP}回を超えました。「止」の条件を確かめてください"
            ))
            .into());
        }
        acc = phrase_sync(interpreter, body.clone(), acc)?;
        crate::values::check_nesting(&acc)?;
    }
    Ok(acc)
}

impl Machine<'_> {
    fn action(&mut self, action: impl FnOnce(&mut Machine<'_>) -> Result<(), Signal> + 'static) {
        self.tasks.push(Task::Action(Box::new(action)));
    }

    fn guard(&mut self, guard: impl FnOnce(&mut Machine<'_>) + 'static) {
        self.tasks.push(Task::Guard(Box::new(guard)));
    }

    fn take(&mut self) -> Result<Arr, Signal> {
        self.value
            .take()
            .ok_or_else(|| AzaError::new("計算の途中の値がありません").into())
    }

    fn finish(&mut self, value: Arr) {
        self.value = Some(value);
    }

    fn span(&mut self, node: Node) {
        self.guard(move |machine| {
            if let Some(signal) = machine.signal.take() {
                machine.signal = Some(place_error(signal, &node));
            }
        });
    }

    fn eval(&mut self, node: Node) {
        self.action(move |machine| {
            match node {
                Node::Expr { subject, steps } => {
                    machine.action(move |machine| {
                        let subject = machine.take()?;
                        machine.start_steps(steps, subject);
                        Ok(())
                    });
                    machine.eval(*subject);
                }
                Node::Group { expr } => machine.eval(*expr),
                Node::ItemList { items } => machine.items(Rc::new(items), 0, Vec::new()),
                Node::MathOp { op, left, right } => {
                    if op == "負" {
                        machine.math_right(op, Arr::scalar(Elem::int(0)));
                        machine.eval(*right);
                    } else {
                        let left = left.ok_or_else(|| AzaError::new("計算の左の値がありません"))?;
                        machine.action(move |machine| {
                            let a = machine.take()?;
                            machine.math_right(op, a);
                            machine.eval(*right);
                            Ok(())
                        });
                        machine.eval(*left);
                    }
                }
                simple => {
                    let value = machine.interpreter.value(&simple)?;
                    machine.finish(value);
                }
            }
            Ok(())
        });
    }

    fn math_right(&mut self, op: String, a: Arr) {
        self.action(move |machine| {
            let b = machine.take()?;
            let name = crate::parser::math_name(&op)
                .ok_or_else(|| AzaError::new("知らない計算記号です"))?;
            machine.finish(primitives::apply(name, &a, Some(&b))?);
            Ok(())
        });
    }

    fn items(&mut self, items: Rc<Vec<Node>>, index: usize, mut data: Vec<Elem>) {
        self.action(move |machine| {
            if index == items.len() {
                machine.finish(Arr::vector(data));
            } else {
                let node = items[index].clone();
                let scalar = matches!(node, Node::Strand { .. });
                machine.action(move |machine| {
                    let value = machine.take()?;
                    data.push(if scalar {
                        pack(value)
                    } else {
                        Elem::Array(Box::new(value))
                    });
                    machine.items(items, index + 1, data);
                    Ok(())
                });
                machine.eval(node);
            }
            Ok(())
        });
    }

    fn start_steps(&mut self, steps: Vec<Node>, subject: Arr) {
        self.interpreter.subjects.push(subject.clone());
        self.guard(|machine| {
            machine.interpreter.subjects.pop();
        });
        self.finish(subject.clone());
        if self.interpreter.trace.is_none() {
            match super::fuse_range_sum(&steps, &subject, self.interpreter) {
                Ok(Some(fused)) => {
                    self.finish(fused);
                    return;
                }
                Ok(None) => (),
                Err(signal) => {
                    self.signal = Some(signal);
                    return;
                }
            }
        }
        self.steps(Rc::new(steps), 0, subject);
    }

    fn steps(&mut self, steps: Rc<Vec<Node>>, index: usize, subject: Arr) {
        self.action(move |machine| {
            // 1つ前の手順の結果を記録する（書き下しの途中のまとまり用。ふだんは何もしない）
            if index > 0 && machine.interpreter.trace.is_some() {
                if let Some(value) = machine.value.clone() {
                    machine.interpreter.trace_step(&steps[index - 1], &value);
                }
            }
            if index == steps.len() {
                return Ok(());
            }
            let acc = machine.take()?;
            crate::values::check_nesting(&acc)?;
            let step = steps[index].clone();
            machine.steps(steps, index + 1, subject.clone());
            machine.span(step.clone());
            machine.step(step, acc, subject);
            Ok(())
        });
    }

    fn step(&mut self, node: Node, acc: Arr, subject: Arr) {
        self.action(move |machine| {
            match node {
                Node::Step { function, right } => {
                    if let Some(right) = right {
                        machine.apply_right(*function, acc);
                        if matches!(right.as_ref(), Node::Phrase { .. }) {
                            machine.phrase(*right, subject);
                        } else {
                            machine.eval(*right);
                        }
                    } else {
                        machine.apply(*function, acc, None);
                    }
                }
                Node::Loop {
                    count, body, until, ..
                } => {
                    if machine.root_execution {
                        let value = loop_sync(
                            machine.interpreter,
                            count.map(|node| *node),
                            *body,
                            until.map(|node| *node),
                            acc,
                        )?;
                        machine.finish(value);
                        return Ok(());
                    }
                    let loop_state = LoopState {
                        body: *body,
                        until: until.map(|n| *n),
                        count: None,
                        done: 0,
                        acc,
                    };
                    if let Some(count) = count {
                        machine.action(move |machine| {
                            let count = as_int(&machine.take()?, "回")?;
                            if count < BigInt::from(0) {
                                return Err(AzaError::new(
                                    "「回」の回数にマイナスの数は使えません",
                                )
                                .into());
                            }
                            machine.loop_check(LoopState {
                                count: Some(count.to_usize().unwrap_or(usize::MAX)),
                                ..loop_state
                            });
                            Ok(())
                        });
                        machine.eval(*count);
                    } else {
                        machine.loop_check(loop_state);
                    }
                }
                Node::Branch {
                    if_true, if_false, ..
                } => machine.branch(*if_true, *if_false, acc, subject)?,
                Node::Pause {} => {
                    machine.interpreter.pauses += 1;
                    if machine.interpreter.pauses <= MAX_PAUSES {
                        machine.interpreter.emit("停", acc.clone());
                    }
                    machine.finish(acc);
                }
                Node::End {} => return Err(Signal::End),
                Node::MathStep { expr } => {
                    machine.interpreter.accs.push(acc);
                    machine.guard(|machine| {
                        machine.interpreter.accs.pop();
                    });
                    machine.eval(*expr);
                }
                _ => return Err(AzaError::new("知らない手順です").into()),
            }
            Ok(())
        });
    }

    fn apply_right(&mut self, function: Node, a: Arr) {
        self.action(move |machine| {
            let b = machine.take()?;
            machine.apply(function, a, Some(b));
            Ok(())
        });
    }

    fn phrase(&mut self, node: Node, subject: Arr) {
        self.action(move |machine| {
            if let Node::Phrase { function, right } = node {
                machine.span((*function).clone());
                if let Some(right) = right {
                    machine.apply_right(*function, subject);
                    machine.eval(*right);
                } else {
                    machine.apply(*function, subject, None);
                }
            } else {
                machine.eval(node);
            }
            Ok(())
        });
    }

    fn loop_check(&mut self, state: LoopState) {
        self.action(move |machine| {
            if let Some(until) = state.until.clone() {
                let subject = state.acc.clone();
                machine.action(move |machine| {
                    if truth(&machine.take()?, "止")? {
                        machine.finish(state.acc);
                    } else {
                        machine.loop_body(state);
                    }
                    Ok(())
                });
                machine.phrase(until, subject);
            } else {
                machine.loop_body(state);
            }
            Ok(())
        });
    }

    fn loop_body(&mut self, mut state: LoopState) {
        self.action(move |machine| {
            if state.count.is_some_and(|count| state.done >= count) {
                machine.finish(state.acc);
                return Ok(());
            }
            state.done += 1;
            if state.done > MAX_LOOP {
                return Err(AzaError::new(format!(
                    "「回」が{MAX_LOOP}回を超えました。「止」の条件を確かめてください"
                ))
                .into());
            }
            let body = state.body.clone();
            let acc = state.acc.clone();
            machine.action(move |machine| {
                state.acc = machine.take()?;
                crate::values::check_nesting(&state.acc)?;
                machine.loop_check(state);
                Ok(())
            });
            machine.phrase(body, acc);
            Ok(())
        });
    }

    fn side(&mut self, node: Node, subject: Arr) {
        match node {
            Node::End {} => self.action(|_| Err(Signal::End)),
            Node::Phrase { .. } => self.phrase(node, subject),
            _ => self.eval(node),
        }
    }

    fn branch(&mut self, yes: Node, no: Node, cond: Arr, subject: Arr) -> Result<(), Signal> {
        if cond.data.len() == 1 && cond.rank() <= 1 {
            self.side(if truth(&cond, "真")? { yes } else { no }, subject);
            return Ok(());
        }
        if matches!(yes, Node::End {}) || matches!(no, Node::End {}) {
            return Err(AzaError::new(
                "「終」を使う条件分岐では、条件を1つ（真か偽）にしてください",
            )
            .into());
        }
        for value in cond.data.iter() {
            check_truth(value, "真")?;
        }
        self.guard(|machine| {
            if matches!(machine.signal, Some(Signal::End)) {
                machine.signal = Some(AzaError::new("条件がリストのときは、真・偽 の中で 終 は使えません（条件を1つにするか、最小・最大 でまとめてください）").into());
            }
        });
        let second_subject = subject.clone();
        self.action(move |machine| {
            let a = machine.take()?;
            machine.action(move |machine| {
                let b = machine.take()?;
                machine.finish(select_sides(&cond, a, b)?);
                Ok(())
            });
            machine.side(no, second_subject);
            Ok(())
        });
        self.side(yes, subject);
        Ok(())
    }

    fn apply(&mut self, function: Node, a: Arr, b: Option<Arr>) {
        self.span(function.clone());
        self.action(move |machine| {
            match function {
                Node::PrimFn { name, .. } => {
                    let value = machine.interpreter.primitive(&name, &a, b.as_ref())?;
                    machine.finish(value);
                }
                Node::DerivedFn { modifier, base, .. } => {
                    machine.derived(&modifier, *base, a, b)?
                }
                Node::InnerFn {
                    reduce, combine, ..
                } => {
                    let b = required_right(b.as_ref(), "内")?.clone();
                    machine.inner(*reduce, *combine, a, b)?;
                }
                Node::NameFn { name, .. } => {
                    let Some(Binding::Function { body, dyadic, line }) =
                        machine.interpreter.env.get(&name).cloned()
                    else {
                        return Err(AzaError::new(format!("【{name}】は操作ではありません")).into());
                    };
                    check_arity(dyadic, b.as_ref(), &name)?;
                    let call = Call {
                        body,
                        a,
                        b,
                        name,
                        are: None,
                        line: Some(line),
                    };
                    if machine.root_execution {
                        let value = run_body(machine.interpreter, call, true)?;
                        machine.finish(value);
                    } else {
                        machine.enter_body(call, true);
                    }
                }
                Node::AnonFn { body, dyadic, .. } => {
                    check_arity(dyadic, b.as_ref(), "その場の操作")?;
                    let are = machine.interpreter.subjects.last().cloned();
                    let call = Call {
                        body: *body,
                        a,
                        b,
                        name: String::new(),
                        are,
                        line: None,
                    };
                    if machine.root_execution {
                        let value = run_body(machine.interpreter, call, false)?;
                        machine.finish(value);
                    } else {
                        machine.enter_body(call, false);
                    }
                }
                _ => return Err(AzaError::new("知らない演算です").into()),
            }
            Ok(())
        });
    }

    fn enter_body(&mut self, call: Call, named: bool) {
        let Call {
            body,
            a,
            b,
            name,
            are,
            line,
        } = call;
        self.action(move |machine| {
            // 数えるのは名前の付いた操作の呼び出しだけ（各（…）などのその場の操作は数えない）
            if named && machine.interpreter.locals.len() >= MAX_DEPTH {
                return Err(AzaError::new(format!(
                    "呼び出しが深すぎます（自分自身を呼ぶ操作は{MAX_DEPTH}段までです）"
                ))
                .into());
            }
            if named {
                machine.interpreter.locals.push(HashMap::new());
            }
            // 中身の記録（プレイグラウンド用）。名前付きの操作は、その定義の行で記録する
            let outer_line = machine.interpreter.trace_line;
            let tracing = machine.interpreter.trace.is_some();
            if tracing {
                if let Some(line) = line {
                    machine.interpreter.trace_line = line;
                }
                let place = if name.is_empty() {
                    crate::expand::anonymous_label(&body)
                } else {
                    format!("【{name}】")
                };
                machine
                    .interpreter
                    .trace_enter(place, outer_line, &a, b.as_ref(), are.as_ref());
            }
            machine.interpreter.args.push((a.clone(), b, are));
            machine.guard(move |machine| {
                if tracing {
                    machine.interpreter.trace_leave();
                    machine.interpreter.trace_line = outer_line;
                }
                machine.interpreter.args.pop();
                if named {
                    machine.interpreter.locals.pop();
                    if let Some(Signal::Error(error)) = &mut machine.signal {
                        error.col = None;
                        error.end = None;
                    }
                }
            });
            match body {
                Node::Train { steps } => machine.start_steps(steps, a),
                Node::Block { lines } => machine.block(Rc::new(lines), 0, name, None),
                other => machine.eval(other),
            }
            Ok(())
        });
    }

    fn block(&mut self, lines: Rc<Vec<Node>>, index: usize, name: String, result: Option<Arr>) {
        self.action(move |machine| {
            if index == lines.len() {
                machine
                    .finish(result.ok_or_else(|| AzaError::new("操作の最後に結果の式が必要です"))?);
                return Ok(());
            }
            let node = lines[index].clone();
            let line = node_line(&node);
            if machine.interpreter.trace.is_some() {
                machine.interpreter.trace_line = line;
            }
            let (expr, local) = match node {
                Node::LocalAssign { name, expr, .. } => (*expr, Some(name)),
                Node::Result { expr, .. } => (*expr, None),
                other => (other, None),
            };
            let context_name = name.clone();
            machine.action(move |machine| {
                let value = machine.take()?;
                let result = if let Some(local) = local {
                    machine.interpreter.trace_local(line, &local, &value);
                    machine
                        .interpreter
                        .locals
                        .last_mut()
                        .ok_or_else(|| AzaError::new("仮の名前は「説」の中で使います"))?
                        .insert(local, value);
                    result
                } else {
                    Some(value)
                };
                machine.block(lines, index + 1, name, result);
                Ok(())
            });
            machine.guard(move |machine| {
                if let Some(Signal::Error(error)) = &mut machine.signal {
                    if error.kind != "stopped"
                        && error.line.is_none()
                        && !error.message.ends_with("行目）")
                    {
                        error
                            .message
                            .push_str(&format!("（【{context_name}】の{}行目）", index + 1));
                    }
                }
            });
            machine.eval(expr);
            Ok(())
        });
    }

    fn derived(
        &mut self,
        modifier: &str,
        base: Node,
        a: Arr,
        b: Option<Arr>,
    ) -> Result<(), Signal> {
        match modifier {
            "総" | "最" | "累" => {
                if b.is_some() {
                    return Err(AzaError::new(format!("「{modifier}」は値を1つとります")).into());
                }
                if a.is_scalar() {
                    self.finish(a);
                    return Ok(());
                }
                let scan = modifier == "累";
                if a.shape.last() == Some(&0) {
                    if !scan {
                        return Err(AzaError::new(format!(
                            "空のリストに「{modifier}{}」は使えません",
                            fn_label(&base)
                        ))
                        .into());
                    }
                    self.finish(a);
                } else {
                    self.reduce_rows(Rc::new(base), Rc::new(a), scan, 0, Vec::new());
                }
            }
            "表" => {
                let b = required_right(b.as_ref(), modifier)?.clone();
                check_size(
                    a.data
                        .len()
                        .checked_mul(b.data.len())
                        .ok_or_else(|| AzaError::new("値が大きすぎます"))?,
                )?;
                let mut shape = a.shape.clone();
                shape.extend(&b.shape);
                self.mapping(MapState {
                    total: 0,
                    base: Rc::new(base),
                    a: Rc::new(a),
                    b: Some(Rc::new(b)),
                    shape,
                    kind: MapKind::Outer,
                    index: 0,
                    data: Vec::new(),
                });
            }
            "自" => {
                if b.is_some() {
                    return Err(AzaError::new("「自」は値を1つとります").into());
                }
                self.apply(base, a.clone(), Some(a));
            }
            "反" => self.apply(base, required_right(b.as_ref(), modifier)?.clone(), Some(a)),
            "試" => {
                let fallback = required_right(b.as_ref(), modifier)?.clone();
                self.guard(move |machine| {
                    if matches!(&machine.signal, Some(Signal::Error(error)) if error.kind != "stopped" && error.kind != "input_required") {
                        machine.signal = None;
                        machine.finish(fallback);
                    }
                });
                self.apply(base, a, None);
            }
            "各" => {
                let mut a = a;
                let mut b = b;
                let shape = if let Some(right) = &mut b {
                    if is_string(&a) && !is_string(right) {
                        a = Arr::scalar(Elem::Array(Box::new(a)));
                    }
                    if is_string(right) && !is_string(&a) {
                        *right = Arr::scalar(Elem::Array(Box::new(right.clone())));
                    }
                    if a.shape == right.shape {
                        a.shape.clone()
                    } else if spreads(&a, right) {
                        right.shape.clone()
                    } else if spreads(right, &a) {
                        a.shape.clone()
                    } else {
                        return Err(AzaError::new(format!(
                            "「各」の左右の形が合いません（{} と {}）",
                            shape_text(&a),
                            shape_text(right)
                        ))
                        .into());
                    }
                } else {
                    a.shape.clone()
                };
                self.mapping(MapState {
                    total: 0,
                    base: Rc::new(base),
                    a: Rc::new(a),
                    b: b.map(Rc::new),
                    shape,
                    kind: MapKind::Each,
                    index: 0,
                    data: Vec::new(),
                });
            }
            "毎" => {
                let b = required_right(b.as_ref(), modifier)?.clone();
                if b.rank() > 1 {
                    return Err(AzaError::new("「毎」の右には、数1つかリストを書きます").into());
                }
                if b.is_scalar() {
                    self.interpreter.positions.push(position(&[], 0));
                    self.guard(|machine| {
                        machine.interpreter.positions.pop();
                    });
                    self.apply(base, a, Some(b));
                } else {
                    let shape = b.shape.clone();
                    self.mapping(MapState {
                        total: 0,
                        base: Rc::new(base),
                        a: Rc::new(a),
                        b: Some(Rc::new(b)),
                        shape,
                        kind: MapKind::Every,
                        index: 0,
                        data: Vec::new(),
                    });
                }
            }
            "非" => {
                let label = format!("非{}", crate::ast::fn_label(&base));
                self.action(move |machine| {
                    let value = machine.take()?;
                    machine.finish(negate(&value, &label)?);
                    Ok(())
                });
                self.apply(base, a, b);
            }
            "前" | "中" | "後" => {
                let rank = a.rank().max(b.as_ref().map(Arr::rank).unwrap_or(0));
                if rank < 2 {
                    self.apply(base, a, b);
                    return Ok(());
                }
                let axis = match modifier {
                    "前" => 0,
                    "後" => rank - 1,
                    _ if rank >= 3 => 1,
                    _ => {
                        return Err(AzaError::new(
                            "「中」は3次元以上の値（表を重ねたもの）に使います",
                        )
                        .into())
                    }
                };
                if axis == rank - 1 {
                    self.apply(base, a, b);
                    return Ok(());
                }
                let mut order: Vec<_> =
                    (0..rank).filter(|axis_index| *axis_index != axis).collect();
                order.push(axis);
                let a = if a.rank() == rank {
                    primitives::permute(&a, &order)?
                } else {
                    a
                };
                let b = b
                    .map(|b| {
                        if b.rank() == rank {
                            primitives::permute(&b, &order)
                        } else {
                            Ok(b)
                        }
                    })
                    .transpose()?;
                let mut inverse = vec![0; rank];
                for (index, original) in order.iter().enumerate() {
                    inverse[*original] = index;
                }
                self.action(move |machine| {
                    let value = machine.take()?;
                    machine.finish(if value.rank() == rank {
                        primitives::permute(&value, &inverse)?
                    } else {
                        value
                    });
                    Ok(())
                });
                self.apply(base, a, b);
            }
            "隣" => {
                let width = a.shape.last().copied().unwrap_or(1);
                let window = b
                    .as_ref()
                    .map(|b| as_int(b, &format!("隣{}", fn_label(&base))))
                    .transpose()?
                    .map(|n| n.to_usize().unwrap_or(0));
                if window.is_some_and(|n| n < 1 || n > width.max(1)) {
                    return Err(AzaError::new(format!(
                        "「隣{}」の右には1〜{width}の数を書きます（いくつずつ計算するか）",
                        fn_label(&base)
                    ))
                    .into());
                }
                let out_width = window
                    .map(|n| width.saturating_sub(n - 1))
                    .unwrap_or_else(|| width.saturating_sub(1));
                let mut shape = a.shape[..a.rank().saturating_sub(1)].to_vec();
                shape.push(out_width);
                self.mapping(MapState {
                    total: 0,
                    base: Rc::new(base),
                    a: Rc::new(a),
                    b: None,
                    shape,
                    kind: MapKind::Neighbor {
                        width,
                        out_width,
                        window,
                    },
                    index: 0,
                    data: Vec::new(),
                });
            }
            _ => {
                return Err(
                    AzaError::new(format!("「{modifier}」は組み合わせの字ではありません")).into(),
                )
            }
        }
        Ok(())
    }

    fn reduce_rows(
        &mut self,
        base: Rc<Node>,
        a: Rc<Arr>,
        scan: bool,
        row: usize,
        mut data: Vec<Elem>,
    ) {
        self.action(move |machine| {
            let width = *a.shape.last().unwrap();
            if row == 0 && matches!(base.as_ref(), Node::PrimFn { name, .. } if name == "足") {
                if let Some(values) = a.data.as_ints() {
                    let rows = values.len() / width;
                    let mut output = Vec::with_capacity(if scan { values.len() } else { rows });
                    let mut safe = true;
                    for row_index in 0..rows {
                        let start = row_index * width;
                        let mut acc = values[start];
                        if scan {
                            output.push(Elem::int(acc));
                        }
                        for value in &values[start + 1..start + width] {
                            let Some(next) = acc.checked_add(*value) else {
                                safe = false;
                                break;
                            };
                            acc = next;
                            if scan {
                                output.push(Elem::int(acc));
                            }
                        }
                        if !safe {
                            break;
                        }
                        if !scan {
                            output.push(Elem::int(acc));
                        }
                    }
                    if safe {
                        if !scan && a.rank() == 1 {
                            machine.finish(item(&output[0]));
                        } else {
                            machine.finish(Arr {
                                shape: if scan {
                                    a.shape.clone()
                                } else {
                                    a.shape[..a.rank() - 1].to_vec()
                                },
                                data: output.into(),
                                kind: String::new(),
                            });
                        }
                        return Ok(());
                    }
                }
            }
            if row * width >= a.data.len() {
                if !scan && a.rank() == 1 {
                    machine.finish(item(data.first().ok_or_else(|| {
                        AzaError::new("空のリストをまとめることはできません")
                    })?));
                } else {
                    machine.finish(Arr {
                        shape: if scan {
                            a.shape.clone()
                        } else {
                            a.shape[..a.rank() - 1].to_vec()
                        },
                        data: data.into(),
                        kind: String::new(),
                    });
                }
                return Ok(());
            }
            let terms = Rc::new(
                (row * width..(row + 1) * width)
                    .map(|index| a.data.get(index).expect("reduction index"))
                    .collect::<Vec<_>>(),
            );
            let acc = terms[0].clone();
            let history = scan.then(|| vec![acc.clone()]);
            let next_base = base.clone();
            machine.action(move |machine| {
                let result = machine.take()?;
                if scan {
                    data.extend(result.data.iter().cloned());
                } else {
                    data.push(pack(result));
                }
                machine.reduce_rows(next_base, a, scan, row + 1, data);
                Ok(())
            });
            machine.series(base, terms, 1, acc, history, 0);
            Ok(())
        });
    }

    fn series(
        &mut self,
        base: Rc<Node>,
        terms: Rc<Vec<Elem>>,
        index: usize,
        acc: Elem,
        mut history: Option<Vec<Elem>>,
        mut total: usize,
    ) {
        self.action(move |machine| {
            // 近道: 足 などの要素どうしの演算は、1つずつ継続を作らずに、ここでまとめて計算する
            let (mut index, mut acc) = (index, acc);
            while index < terms.len() {
                let Some(result) = quick_pair(&base, &acc, &terms[index]) else {
                    break;
                };
                acc = result?;
                if let Some(history) = &mut history {
                    history.push(acc.clone());
                }
                index += 1;
            }
            if index == terms.len() {
                machine.finish(if let Some(history) = history {
                    Arr::vector(history)
                } else {
                    item(&acc)
                });
                return Ok(());
            }
            let x = item(&acc);
            let y = item(&terms[index]);
            let next_base = base.clone();
            machine.action(move |machine| {
                let value = machine.take()?;
                if history.is_some() {
                    crate::values::add_total(&mut total, &value)?;
                }
                let result = pack(value);
                if let Some(history) = &mut history {
                    history.push(result.clone());
                }
                machine.series(next_base, terms, index + 1, result, history, total);
                Ok(())
            });
            machine.apply((*base).clone(), x, Some(y));
            Ok(())
        });
    }

    fn mapping(&mut self, mut state: MapState) {
        self.action(move |machine| {
            let size = state.shape.iter().product::<usize>();
            if state.index == 0
                && matches!(&state.kind, MapKind::Each)
                && state.b.is_none()
                && machine.interpreter.trace.is_none()
                && state.a.data.as_ints().is_some()
            {
                if let Some(operation) = simple_each_integer_op(&state.base) {
                    let values = state.a.data.as_ints().expect("checked integer storage");
                    let mut data = Vec::with_capacity(values.len());
                    let mut fits = true;
                    for (index, value) in values.iter().enumerate() {
                        if index > 0 && index % 65_536 == 0 {
                            machine.interpreter.check_stop()?;
                        }
                        if let Some(value) = operation.apply(*value) {
                            data.push(value);
                        } else {
                            fits = false;
                            break;
                        }
                    }
                    if fits {
                        machine.finish(Arr {
                            shape: state.shape,
                            data: crate::values::Data::ints(data),
                            kind: String::new(),
                        });
                        return Ok(());
                    }
                }
            }
            // 近道: 表足・各掛 などで、うしろが要素どうしの演算（足 など）なら、
            // 1つずつ継続を作らずにここでまとめて計算する（入れ子に出会ったら、そこから普通の道に戻る）
            if machine.interpreter.trace.is_none() {
                if let Some(b) = state.b.clone() {
                    let (a_len, b_len) = (state.a.data.len(), b.data.len());
                    while state.index < size {
                        let (x, y) = match state.kind {
                            MapKind::Outer => (state.index / b_len, state.index % b_len),
                            MapKind::Each => (
                                if a_len == 1 { 0 } else { state.index },
                                if b_len == 1 { 0 } else { state.index },
                            ),
                            _ => break,
                        };
                        let (Some(x), Some(y)) = (state.a.data.get(x), b.data.get(y)) else {
                            break;
                        };
                        let Some(result) = super::quick_pair(&state.base, &x, &y) else {
                            break;
                        };
                        state.data.push(result?);
                        state.index += 1;
                        if state.index.is_multiple_of(65_536) {
                            machine.interpreter.check_stop()?;
                        }
                    }
                }
            }
            if state.index == size {
                machine.finish(Arr {
                    shape: state.shape,
                    data: state.data.into(),
                    kind: String::new(),
                });
                return Ok(());
            }
            let (a, b, pos, window) = match state.kind {
                MapKind::Each => {
                    let a = item(&at(
                        &state.a,
                        if state.a.data.len() == 1 {
                            0
                        } else {
                            state.index
                        },
                    ));
                    let b = state
                        .b
                        .as_ref()
                        .map(|b| item(&at(b, if b.data.len() == 1 { 0 } else { state.index })));
                    (a, b, Some(position(&state.shape, state.index)), None)
                }
                MapKind::Every => {
                    let b = state.b.as_ref().unwrap();
                    (
                        (*state.a).clone(),
                        Some(item(&at(b, state.index))),
                        Some(position(&b.shape, state.index)),
                        None,
                    )
                }
                MapKind::Outer => {
                    let b = state.b.as_ref().unwrap();
                    (
                        item(&at(&state.a, state.index / b.data.len())),
                        Some(item(&at(b, state.index % b.data.len()))),
                        None,
                        None,
                    )
                }
                MapKind::Neighbor {
                    width,
                    out_width,
                    window,
                } => {
                    let start = state.index / out_width * width + state.index % out_width;
                    if let Some(window) = window {
                        let terms: Vec<Elem> =
                            (start..start + window).map(|i| at(&state.a, i)).collect();
                        (Arr::scalar(Elem::int(0)), None, None, Some(terms))
                    } else {
                        (
                            item(&at(&state.a, start + 1)),
                            Some(item(&at(&state.a, start))),
                            None,
                            None,
                        )
                    }
                }
            };
            let base = state.base.clone();
            machine.action(move |machine| {
                let value = machine.take()?;
                crate::values::add_total(&mut state.total, &value)?;
                state.data.push(pack(value));
                state.index += 1;
                machine.mapping(state);
                Ok(())
            });
            if let Some(position) = pos {
                machine.interpreter.positions.push(position);
                machine.guard(|machine| {
                    machine.interpreter.positions.pop();
                });
            }
            if let Some(terms) = window {
                let acc = terms[0].clone();
                machine.series(base, Rc::new(terms), 1, acc, None, 0);
            } else {
                machine.apply((*base).clone(), a, b);
            }
            Ok(())
        });
    }

    fn inner(&mut self, reduce: Node, combine: Node, a: Arr, b: Arr) -> Result<(), Signal> {
        let label = format!("内{}{}", fn_label(&reduce), fn_label(&combine));
        if a.rank() > 2 || b.rank() > 2 {
            return Err(AzaError::new(format!("「{label}」は表（2次元）までに使えます")).into());
        }
        let rows = if a.rank() == 2 { a.shape[0] } else { 1 };
        let width = if a.rank() == 2 {
            a.shape[1]
        } else {
            a.data.len()
        };
        let columns = if b.rank() == 2 { b.shape[1] } else { 1 };
        let right_width = if b.rank() == 2 {
            b.shape[0]
        } else {
            b.data.len()
        };
        if columns > 0 && right_width != width {
            return Err(AzaError::new(format!(
                "「{label}」の左右の長さが合いません（{} と {}）",
                shape_text(&a),
                shape_text(&b)
            ))
            .into());
        }
        let size = rows
            .checked_mul(columns)
            .ok_or_else(|| AzaError::new("値が大きすぎます"))?;
        check_size(size)?;
        let mut shape = Vec::new();
        if a.rank() == 2 {
            shape.push(rows);
        }
        if b.rank() == 2 {
            shape.push(columns);
        }
        self.inner_cells(InnerState {
            reduce: Rc::new(reduce),
            combine: Rc::new(combine),
            a: Rc::new(a),
            b: Rc::new(b),
            width,
            columns,
            size,
            shape,
            index: 0,
            data: Vec::new(),
        });
        Ok(())
    }

    fn inner_cells(&mut self, mut state: InnerState) {
        self.action(move |machine| {
            if state.index == state.size {
                machine.finish(Arr {
                    shape: state.shape,
                    data: state.data.into(),
                    kind: String::new(),
                });
                return Ok(());
            }
            if state.width == 0 {
                return Err(AzaError::new(format!(
                    "空のリストに「内{}{}」は使えません",
                    fn_label(&state.reduce),
                    fn_label(&state.combine)
                ))
                .into());
            }
            let row = state.index / state.columns;
            let column = state.index % state.columns;
            let mut xs = Vec::with_capacity(state.width);
            let mut ys = Vec::with_capacity(state.width);
            for index in 0..state.width {
                xs.push(at(&state.a, row * state.width + index));
                ys.push(at(
                    &state.b,
                    if state.b.rank() == 2 {
                        index * state.columns + column
                    } else {
                        index
                    },
                ));
            }
            let reduce = state.reduce.clone();
            let combine = state.combine.clone();
            machine.action(move |machine| {
                let terms = Rc::new(machine.take()?.data.to_vec());
                let acc = terms[0].clone();
                machine.action(move |machine| {
                    state.data.push(pack(machine.take()?));
                    state.index += 1;
                    machine.inner_cells(state);
                    Ok(())
                });
                machine.series(reduce, terms, 1, acc, None, 0);
                Ok(())
            });
            machine.inner_terms(combine, Rc::new(xs), Rc::new(ys), 0, Vec::new());
            Ok(())
        });
    }

    fn inner_terms(
        &mut self,
        base: Rc<Node>,
        xs: Rc<Vec<Elem>>,
        ys: Rc<Vec<Elem>>,
        index: usize,
        mut data: Vec<Elem>,
    ) {
        self.action(move |machine| {
            if index == xs.len() {
                machine.finish(Arr::vector(data));
                return Ok(());
            }
            let a = item(&xs[index]);
            let b = item(&ys[index]);
            let next_base = base.clone();
            machine.action(move |machine| {
                data.push(pack(machine.take()?));
                machine.inner_terms(next_base, xs, ys, index + 1, data);
                Ok(())
            });
            machine.apply((*base).clone(), a, Some(b));
            Ok(())
        });
    }
}

struct LoopState {
    body: Node,
    until: Option<Node>,
    count: Option<usize>,
    done: usize,
    acc: Arr,
}
enum MapKind {
    Each,
    Every,
    Outer,
    Neighbor {
        width: usize,
        out_width: usize,
        window: Option<usize>,
    },
}
struct MapState {
    base: Rc<Node>,
    a: Rc<Arr>,
    b: Option<Rc<Arr>>,
    shape: Vec<usize>,
    kind: MapKind,
    index: usize,
    data: Vec<Elem>,
    /// 集めた結果の要素の総数（入れ子の中も数える）
    total: usize,
}
struct InnerState {
    reduce: Rc<Node>,
    combine: Rc<Node>,
    a: Rc<Arr>,
    b: Rc<Arr>,
    width: usize,
    columns: usize,
    size: usize,
    shape: Vec<usize>,
    index: usize,
    data: Vec<Elem>,
}

/// i 番目の要素（整数・小数・文字だけの配列を、要素の一覧に広げずに読む）
fn at(a: &Arr, i: usize) -> Elem {
    a.data.get(i).expect("index within the array")
}

fn select_sides(cond: &Arr, mut a: Arr, mut b: Arr) -> EvalResult {
    let chars = is_string(&a) && a.data.len() == 1 && is_string(&b) && b.data.len() == 1;
    if chars {
        a = Arr::scalar(a.data[0].clone());
        b = Arr::scalar(b.data[0].clone());
    } else {
        if is_string(&a) {
            a = Arr::scalar(Elem::Array(Box::new(a)));
        }
        if is_string(&b) {
            b = Arr::scalar(Elem::Array(Box::new(b)));
        }
    }
    for (value, word) in [(&a, "真"), (&b, "偽")] {
        if !value.is_scalar() && value.shape != cond.shape {
            return Err(AzaError::new(format!(
                "条件が{}のとき、「{word}」の値は1つか、同じ形にしてください（{}がありました）",
                shape_text(cond),
                shape_text(value)
            ))
            .into());
        }
    }
    let mut data = Vec::with_capacity(cond.data.len());
    for (index, condition) in cond.data.iter().enumerate() {
        let value = if check_truth(condition, "真")? {
            &a
        } else {
            &b
        };
        data.push(at(value, if value.is_scalar() { 0 } else { index }));
    }
    Ok(Arr {
        shape: cond.shape.clone(),
        data: data.into(),
        kind: if chars { "text".into() } else { String::new() },
    })
}

/// 非: 結果の真（1）と偽（0）を入れ替える。入れ子の中まで入る。1 と 0 以外があればエラー。
fn negate(value: &Arr, label: &str) -> Result<Arr, AzaError> {
    fn flip(x: &Elem, label: &str) -> Result<Elem, AzaError> {
        match x {
            Elem::Array(inner) => Ok(Elem::Array(Box::new(negate(inner, label)?))),
            _ if *x == Elem::int(0) => Ok(Elem::int(1)),
            _ if *x == Elem::int(1) => Ok(Elem::int(0)),
            _ => Err(AzaError::new(format!(
                "「{label}」は、結果が真（1）か偽（0）になる演算（等・以上・且 など）に使います（{}がありました）",
                show_elem(x)
            ))),
        }
    }
    let data = value
        .data
        .iter()
        .map(|x| flip(x, label))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Arr::new(value.shape.clone(), data))
}
