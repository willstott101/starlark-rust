/*
 * Copyright 2019 The Starlark in Rust Authors.
 * Copyright (c) Facebook, Inc. and its affiliates.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     https://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! Compile function calls.

use crate::{
    codemap::{Span, Spanned},
    collections::symbol_map::Symbol,
    environment::FrozenModuleRef,
    eval::{
        compiler::{
            scope::{CstArgument, CstExpr},
            Compiler,
        },
        fragment::expr::{ExprCompiledValue, MaybeNot},
        FrozenDef,
    },
    gazebo::prelude::SliceExt,
    syntax::ast::{ArgumentP, AstString, ExprP},
    values::{
        string::interpolation::parse_format_one, AttrType, FrozenStringValue, FrozenValue,
        ValueLike,
    },
};

#[derive(Default, Clone, Debug)]
pub(crate) struct ArgsCompiledValue {
    pub(crate) pos_named: Vec<Spanned<ExprCompiledValue>>,
    /// Named arguments compiled.
    ///
    /// Note names are guaranteed to be unique here because names are validated in AST:
    /// named arguments in [`Expr::Call`] are unique.
    pub(crate) names: Vec<(Symbol, FrozenStringValue)>,
    pub(crate) args: Option<Spanned<ExprCompiledValue>>,
    pub(crate) kwargs: Option<Spanned<ExprCompiledValue>>,
}

#[derive(Clone, Debug)]
pub(crate) enum CallCompiled {
    Call(Box<(Spanned<ExprCompiledValue>, ArgsCompiledValue)>),
    Frozen(Box<(Option<FrozenValue>, FrozenValue, ArgsCompiledValue)>),
    Method(Box<(Spanned<ExprCompiledValue>, Symbol, ArgsCompiledValue)>),
}

impl Spanned<CallCompiled> {
    pub(crate) fn optimize_on_freeze(&self, module: &FrozenModuleRef) -> ExprCompiledValue {
        ExprCompiledValue::Call(self.map(|call| match *call {
            CallCompiled::Call(box (ref fun, ref args)) => {
                let fun = fun.optimize_on_freeze(module);
                let args = args.optimize_on_freeze(module);
                if let Spanned {
                    node: ExprCompiledValue::Value(fun),
                    ..
                } = fun
                {
                    CallCompiled::Frozen(box (None, fun, args))
                } else {
                    CallCompiled::Call(box (fun, args))
                }
            }
            CallCompiled::Frozen(box (this, fun, ref args)) => {
                let args = args.optimize_on_freeze(module);
                CallCompiled::Frozen(box (this, fun, args))
            }
            CallCompiled::Method(box (ref this, ref field, ref args)) => {
                let this = this.optimize_on_freeze(module);
                let field = field.clone();
                let args = args.optimize_on_freeze(module);
                CallCompiled::Method(box (this, field, args))
            }
        }))
    }
}

impl ArgsCompiledValue {
    pub(crate) fn pos_only(&self) -> Option<&[Spanned<ExprCompiledValue>]> {
        if self.names.is_empty() && self.args.is_none() && self.kwargs.is_none() {
            Some(&self.pos_named)
        } else {
            None
        }
    }

    fn optimize_on_freeze(&self, module: &FrozenModuleRef) -> ArgsCompiledValue {
        let ArgsCompiledValue {
            ref pos_named,
            ref names,
            ref args,
            ref kwargs,
        } = *self;
        ArgsCompiledValue {
            pos_named: pos_named.map(|p| p.optimize_on_freeze(module)),
            names: names.clone(),
            args: args.as_ref().map(|a| a.optimize_on_freeze(module)),
            kwargs: kwargs.as_ref().map(|a| a.optimize_on_freeze(module)),
        }
    }
}

impl Compiler<'_> {
    fn args(&mut self, args: Vec<CstArgument>) -> ArgsCompiledValue {
        let mut res = ArgsCompiledValue::default();
        for x in args {
            match x.node {
                ArgumentP::Positional(x) => res.pos_named.push(self.expr(x)),
                ArgumentP::Named(name, value) => {
                    let fv = self
                        .module_env
                        .frozen_heap()
                        .alloc_string_value(name.node.as_str());
                    res.names.push((Symbol::new(&name.node), fv));
                    res.pos_named.push(self.expr(value));
                }
                ArgumentP::Args(x) => res.args = Some(self.expr(x)),
                ArgumentP::KwArgs(x) => res.kwargs = Some(self.expr(x)),
            }
        }
        res
    }

    fn expr_call_fun_frozen_no_special(
        &mut self,
        span: Span,
        this: Option<FrozenValue>,
        fun: FrozenValue,
        args: Vec<CstArgument>,
    ) -> ExprCompiledValue {
        let args = self.args(args);
        ExprCompiledValue::Call(Spanned {
            span,
            node: CallCompiled::Frozen(box (this, fun, args)),
        })
    }

    fn expr_call_fun_frozen(
        &mut self,
        span: Span,
        left: FrozenValue,
        mut args: Vec<CstArgument>,
    ) -> ExprCompiledValue {
        let one_positional = args.len() == 1 && args[0].is_positional();
        if left == self.constants.fn_type && one_positional {
            self.fn_type(args.pop().unwrap().node.into_expr())
        } else if left == self.constants.fn_len && one_positional {
            let x = self.expr(args.pop().unwrap().node.into_expr());
            ExprCompiledValue::Len(box x)
        } else {
            if one_positional {
                // Try to inline a function like `lambda x: type(x) == "y"`.
                if let Some(left) = left.downcast_ref::<FrozenDef>() {
                    if let Some(t) = &left.def_info.returns_type_is {
                        assert!(args.len() == 1);
                        let arg = args.pop().unwrap();
                        return match arg.node {
                            ArgumentP::Positional(e) => {
                                ExprCompiledValue::TypeIs(box self.expr(e), *t, MaybeNot::Id)
                            }
                            _ => unreachable!(),
                        };
                    }
                }
            }
            self.expr_call_fun_frozen_no_special(span, None, left, args)
        }
    }

    fn expr_call_fun_compiled(
        &mut self,
        span: Span,
        left: Spanned<ExprCompiledValue>,
        args: Vec<CstArgument>,
    ) -> ExprCompiledValue {
        if let Some(left) = left.as_value() {
            self.expr_call_fun_frozen(span, left, args)
        } else {
            let args = self.args(args);
            ExprCompiledValue::Call(Spanned {
                span,
                node: CallCompiled::Call(box (left, args)),
            })
        }
    }

    fn expr_call_method(
        &mut self,
        span: Span,
        e: CstExpr,
        s: AstString,
        mut args: Vec<CstArgument>,
    ) -> ExprCompiledValue {
        let e = self.expr(e);

        // Optimize `"aaa{}bbb".format(arg)`.
        if let Some(e) = e.as_string() {
            if s.node == "format" && args.len() == 1 {
                if let ArgumentP::Positional(..) = args[0].node {
                    if let Some((before, after)) = parse_format_one(&e) {
                        let before = self.module_env.frozen_heap().alloc_string_value(&before);
                        let after = self.module_env.frozen_heap().alloc_string_value(&after);
                        let arg = match args.pop().unwrap().node {
                            ArgumentP::Positional(arg) => arg,
                            _ => unreachable!(),
                        };
                        assert!(args.is_empty());
                        let arg = self.expr(arg);
                        return ExprCompiledValue::FormatOne(box (before, arg, after));
                    }
                }
            }
        }

        let s = Symbol::new(&s.node);
        if let Some(e) = e.as_value() {
            if let Some((at, fun)) = self.compile_time_getattr(e, &s) {
                let this = match at {
                    AttrType::Field => None,
                    AttrType::Method => Some(e),
                };
                return self.expr_call_fun_frozen_no_special(span, this, fun, args);
            }
        }
        let args = self.args(args);
        ExprCompiledValue::Call(Spanned {
            span,
            node: CallCompiled::Method(box (e, s, args)),
        })
    }

    pub(crate) fn expr_call(
        &mut self,
        span: Span,
        left: CstExpr,
        args: Vec<CstArgument>,
    ) -> ExprCompiledValue {
        match left.node {
            ExprP::Dot(box e, s) => self.expr_call_method(span, e, s, args),
            _ => {
                let expr = self.expr(left);
                self.expr_call_fun_compiled(span, expr, args)
            }
        }
    }
}
