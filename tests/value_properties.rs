use azaori::{
    ast::Node,
    error::AzaError,
    interpreter::Interpreter,
    primitives::{apply, permute, scalar_dyad},
    values::{Arr, Elem},
};
use rand::{rngs::StdRng, Rng, SeedableRng};
use serde_json::{json, Value};

fn outcome(result: Result<Arr, AzaError>) -> Value {
    match result {
        Ok(a) => json!({"value":a.to_json()}),
        Err(e) => json!({"error":e.message}),
    }
}

#[test]
fn array_arithmetic_matches_scalar_oracle_without_mutating_inputs() {
    let mut rng = StdRng::seed_from_u64(7);
    let pool = vec![
        Elem::int(0),
        Elem::int(1),
        Elem::int(-1),
        Elem::int(2),
        Elem::int(3),
        Elem::Float(0.5),
        Elem::Float(-2.5),
        Elem::Float(1.0),
        Elem::Float(0.0),
        Elem::Int("1000000000000000000000000000000".parse().unwrap()),
        Elem::int(7),
        Elem::Float(1e308),
    ];
    for op in [
        "足", "引", "掛", "割", "余", "大", "小", "等", "異", "超", "未満", "以上", "以下", "且",
        "又",
    ] {
        for _ in 0..200 {
            let n = rng.gen_range(1..=4);
            let a = Arr::vector(
                (0..n)
                    .map(|_| pool[rng.gen_range(0..pool.len())].clone())
                    .collect(),
            );
            let b = if rng.gen_bool(0.7) {
                Arr::vector(
                    (0..n)
                        .map(|_| pool[rng.gen_range(0..pool.len())].clone())
                        .collect(),
                )
            } else {
                Arr::scalar(pool[rng.gen_range(0..pool.len())].clone())
            };
            let saved = (a.to_json(), b.to_json());
            let scalar = a
                .data
                .iter()
                .enumerate()
                .map(|(i, x)| scalar_dyad(op, x, &b.data[if b.is_scalar() { 0 } else { i }]))
                .collect::<Result<Vec<_>, _>>()
                .map(Arr::vector);
            assert_eq!(
                outcome(apply(op, &a, Some(&b))),
                outcome(scalar),
                "{op}: {a:?} {b:?}"
            );
            assert_eq!((a.to_json(), b.to_json()), saved);
        }
    }
}

fn derived(modifier: &str, op: &str, values: Vec<String>) -> Result<Arr, AzaError> {
    let program = Node::Program {
        body: vec![Node::Show {
            line: 1,
            expr: Box::new(Node::Expr {
                subject: Box::new(Node::Strand { values }),
                steps: vec![Node::Step {
                    function: Box::new(Node::DerivedFn {
                        modifier: modifier.into(),
                        base: Box::new(Node::PrimFn {
                            name: op.into(),
                            col: -1,
                            end: -1,
                        }),
                        col: -1,
                        end: -1,
                    }),
                    right: None,
                }],
            }),
        }],
    };
    let mut interpreter = Interpreter::default();
    interpreter
        .run(&program)
        .map(|results| results[0].1.clone())
}

#[test]
fn folds_and_scans_match_left_associative_scalar_steps() {
    let mut rng = StdRng::seed_from_u64(77);
    let pool = [
        ("0", Elem::int(0)),
        ("1", Elem::int(1)),
        ("-1", Elem::int(-1)),
        ("3", Elem::int(3)),
        ("0.5", Elem::Float(0.5)),
        ("-2.5", Elem::Float(-2.5)),
        ("1.0", Elem::Float(1.0)),
        (
            "1000000000000000000000000000000",
            Elem::Int("1000000000000000000000000000000".parse().unwrap()),
        ),
        ("1e308", Elem::Float(1e308)),
    ];
    for op in [
        "足", "引", "掛", "割", "余", "大", "小", "等", "異", "超", "未満", "以上", "以下", "且",
        "又",
    ] {
        for _ in 0..200 {
            let n = rng.gen_range(1..=4);
            let selected: Vec<_> = (0..n)
                .map(|_| &pool[rng.gen_range(0..pool.len())])
                .collect();
            let numbers: Vec<String> = selected.iter().map(|(s, _)| s.to_string()).collect();
            let mut acc = selected[0].1.clone();
            let mut scan = vec![acc.clone()];
            let mut failed = None;
            for (_, x) in selected.iter().skip(1) {
                match scalar_dyad(op, &acc, x) {
                    Ok(next) => {
                        acc = next;
                        scan.push(acc.clone());
                    }
                    Err(e) => {
                        failed = Some(e);
                        break;
                    }
                }
            }
            let fold_expected = if let Some(ref e) = failed {
                Err(AzaError::new(&e.message))
            } else {
                Ok(Arr::scalar(acc))
            };
            let scan_expected = if let Some(e) = failed {
                Err(e)
            } else if n == 1 {
                Ok(Arr::scalar(scan[0].clone()))
            } else {
                Ok(Arr::vector(scan))
            };
            assert_eq!(
                outcome(derived("総", op, numbers.clone())),
                outcome(fold_expected),
                "総{op}: {numbers:?}"
            );
            assert_eq!(
                outcome(derived("累", op, numbers.clone())),
                outcome(scan_expected),
                "累{op}: {numbers:?}"
            );
        }
    }
}

#[test]
fn every_three_dimensional_permutation_matches_coordinate_definition() {
    for shape in [
        vec![2, 3, 4],
        vec![1, 3, 2],
        vec![0, 3, 2],
        vec![2, 0, 4],
        vec![2, 3, 0],
    ] {
        let a = Arr::new(
            shape.clone(),
            (0..shape.iter().product::<usize>())
                .map(Elem::int)
                .collect(),
        );
        for order in [
            [0, 1, 2],
            [2, 1, 0],
            [1, 0, 2],
            [0, 2, 1],
            [2, 0, 1],
            [1, 2, 0],
        ] {
            let result = permute(&a, &order).unwrap();
            for i in 0..result.shape[0] {
                for j in 0..result.shape[1] {
                    for k in 0..result.shape[2] {
                        let mut original = [0; 3];
                        for (axis, index) in [i, j, k].iter().enumerate() {
                            original[order[axis]] = *index;
                        }
                        let old = (original[0] * shape[1] + original[1]) * shape[2] + original[2];
                        let new = (i * result.shape[1] + j) * result.shape[2] + k;
                        assert_eq!(result.data[new], Elem::int(old));
                    }
                }
            }
            let mut inverse = [0; 3];
            for (i, axis) in order.iter().enumerate() {
                inverse[*axis] = i;
            }
            assert_eq!(permute(&result, &inverse).unwrap(), a);
        }
    }
}

#[test]
fn strings_preserve_empty_kind_through_selection_and_rotation() {
    for source in ["", "日本語", "a🙂b"] {
        let a = Arr::text(source);
        let empty = apply("取", &a, Some(&Arr::scalar(Elem::int(0)))).unwrap();
        assert_eq!(empty.kind, "text");
        for n in -8..=8 {
            let rotated = apply("移", &a, Some(&Arr::scalar(Elem::int(n)))).unwrap();
            assert_eq!(
                apply("移", &rotated, Some(&Arr::scalar(Elem::int(-n))))
                    .unwrap()
                    .to_json(),
                a.to_json()
            );
        }
    }
}
