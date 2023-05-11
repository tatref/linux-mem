use anyhow::{bail, Context, Result};
use log::{debug, warn};
use std::ffi::OsString;

use procfs::process::Process;

use crate::process_tree::ProcessTree;

pub trait Filter: std::fmt::Debug {
    fn eval(&self, p: &Process, tree: &ProcessTree) -> bool;
}

#[derive(Debug)]
struct NotFilter {
    inner: Box<dyn Filter>,
}
impl Filter for NotFilter {
    fn eval(&self, p: &Process, tree: &ProcessTree) -> bool {
        !self.inner.eval(p, tree)
    }
}

#[derive(Debug)]
struct TrueFilter;
impl Filter for TrueFilter {
    fn eval(&self, _p: &Process, _: &ProcessTree) -> bool {
        true
    }
}

#[derive(Debug)]
struct FalseFilter;
impl Filter for FalseFilter {
    fn eval(&self, _p: &Process, _: &ProcessTree) -> bool {
        false
    }
}

#[derive(Debug)]
struct AndFilter {
    pub children: Vec<Box<dyn Filter>>,
}
impl Filter for AndFilter {
    fn eval(&self, p: &Process, tree: &ProcessTree) -> bool {
        self.children.iter().all(|child| child.eval(p, tree))
    }
}

#[derive(Debug)]
struct OrFilter {
    pub children: Vec<Box<dyn Filter>>,
}
impl Filter for OrFilter {
    fn eval(&self, p: &Process, tree: &ProcessTree) -> bool {
        self.children.iter().any(|child| child.eval(p, tree))
    }
}

#[derive(Debug)]
struct CommFilter {
    pub comm: String,
}
impl Filter for CommFilter {
    fn eval(&self, p: &Process, _: &ProcessTree) -> bool {
        match p.stat() {
            Ok(stat) => stat.comm == self.comm,
            Err(_) => false,
        }
    }
}

#[derive(Debug)]
struct UidFilter {
    pub uid: u32,
}
impl Filter for UidFilter {
    fn eval(&self, p: &Process, _: &ProcessTree) -> bool {
        match p.uid() {
            Ok(uid) => uid == self.uid,
            Err(_) => false,
        }
    }
}

#[derive(Debug)]
struct DescendantsFilter {
    pub pid: i32,
}
impl Filter for DescendantsFilter {
    fn eval(&self, p: &Process, tree: &ProcessTree) -> bool {
        let procs = tree.descendants(self.pid);
        procs.contains(&p.pid)
    }
}

#[derive(Debug)]
struct PidFilter {
    pub pid: i32,
}
impl Filter for PidFilter {
    fn eval(&self, p: &Process, _: &ProcessTree) -> bool {
        self.pid == p.pid
    }
}

#[derive(Debug)]
pub struct EnvironKFilter {
    pub key: String,
}
impl Filter for EnvironKFilter {
    fn eval(&self, p: &Process, _: &ProcessTree) -> bool {
        match p.environ() {
            Ok(e) => e.get(&OsString::from(&self.key)).is_some(),
            Err(_) => false,
        }
    }
}

#[derive(Debug)]
pub struct EnvironKVFilter {
    pub key: String,
    pub value: String,
}
impl Filter for EnvironKVFilter {
    fn eval(&self, p: &Process, _: &ProcessTree) -> bool {
        match p.environ() {
            Ok(e) => e.get(&OsString::from(&self.key)) == Some(&OsString::from(&self.value)),
            Err(_) => false,
        }
    }
}

/// uid(0)
/// env(ORACLE_SID, PROD)
/// or(uid(0), uid(1000))
/// and(env(ORACLE_SID, PROD), uid(1000))
/// descendant(1234)
/// limitations:
/// - vars can't contain ()
pub fn parse(input: &str) -> Result<(Box<dyn Filter>, usize)> {
    debug!("Parsing: {input:?}");

    let opening = input
        .find('(')
        .with_context(|| "Missing opening parenthesis")?;

    let name: String = input.chars().take(opening).collect();
    debug!("operator: {:?}", name);

    fn find_match_par(input: &str, idx: usize) -> Result<usize> {
        let mut counter = 0;
        let mut iter = input.char_indices().skip(idx);
        loop {
            let Some((idx, c)) = iter.next() else {
                bail!("Unbalanced parenthesis at end of string");
            };

            match c {
                '(' => counter += 1,
                ')' => counter -= 1,
                _ => (),
            }

            if counter < 0 {
                bail!("Too many closing parenthesis");
            }
            if counter == 0 {
                return Ok(idx);
            }
        }
    }

    let closing = find_match_par(input, opening)?;
    let inner: String = input
        .chars()
        .skip(opening + 1)
        .take(closing - opening - 1)
        .collect();
    debug!("opening, closing: {} {}", opening, closing);
    debug!("inner: {inner:?}");

    if closing + 1 != input.chars().count() {
        //warn!("Didn't consume whole filter");
        //info!("Parsing: {input:?}");
        //info!("opening, closing: {} {}", opening, closing);
        //info!("inner: {inner:?}");
    }

    let ate = closing + 1;
    debug!("ate: {ate:?}");

    match name.as_str() {
        "and" | "or" => {
            let mut from = 0;
            let mut inners = Vec::new();
            loop {
                let Ok((parsed_inner, inner_ate)) = parse(&inner[from..]) else {
                    bail!("Can't parse {:?}", &inner[from..]);
                };
                inners.push(parsed_inner);
                from += inner_ate + 1;

                if from > inner.chars().count() {
                    break;
                }
            }
            if inners.is_empty() {
                bail!("Empty filter for {name:?}");
            }

            if name == "and" {
                Ok((Box::new(AndFilter { children: inners }), ate))
            } else if name == "or" {
                Ok((Box::new(OrFilter { children: inners }), ate))
            } else {
                unreachable!()
            }
        }
        "not" => {
            let (parsed_inner, ate_2) = parse(&inner)?;
            if ate_2 < inner.chars().count() {
                warn!("Ignored garbage {:?}", &inner[ate_2..]);
            }
            Ok((
                Box::new(NotFilter {
                    inner: parsed_inner,
                }),
                ate,
            ))
        }
        "descendants" => {
            let inner = inner
                .parse()
                .with_context(|| "Argument of 'descendants' filter must be a number")?;
            Ok((Box::new(DescendantsFilter { pid: inner }), ate))
        }
        "comm" => Ok((Box::new(CommFilter { comm: inner }), ate)),
        "pid" => {
            let inner = inner
                .parse()
                .with_context(|| "Argument of 'descendant' filter must be a number")?;
            Ok((Box::new(PidFilter { pid: inner }), ate))
        }
        "uid" => {
            let inner = inner
                .parse()
                .with_context(|| "Argument of 'descendant' filter must be a number")?;
            Ok((Box::new(UidFilter { uid: inner }), ate))
        }

        "env_kv" => {
            let mut iter = inner.split(',');
            let key = iter
                .next()
                .with_context(|| "Invalid key for env_kv")?
                .trim()
                .to_string();
            let value = iter
                .next()
                .with_context(|| "Invalid value for env_kv")?
                .trim()
                .to_string();
            Ok((Box::new(EnvironKVFilter { key, value }), ate))
        }
        "env_k" => {
            let key = inner;
            Ok((Box::new(EnvironKFilter { key }), ate))
        }
        "true" => Ok((Box::new(TrueFilter), ate)),
        "false" => Ok((Box::new(FalseFilter), ate)),
        x => bail!("Unknown filter: {x:?}"),
    }
}
