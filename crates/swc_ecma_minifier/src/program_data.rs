use std::collections::hash_map::Entry;

use indexmap::IndexSet;
use rustc_hash::FxHashMap;
use swc_atoms::JsWord;
use swc_common::{
    collections::{AHashMap, AHashSet, ARandomState},
    SyntaxContext,
};
use swc_ecma_ast::*;
use swc_ecma_usage_analyzer::{
    alias::{Access, AccessKind},
    analyzer::{
        analyze_with_storage,
        storage::{ScopeDataLike, Storage, VarDataLike},
        CalleeKind, Ctx, ScopeKind, UsageAnalyzer,
    },
    marks::Marks,
};
use swc_ecma_visit::VisitWith;

pub(crate) fn analyze<N>(n: &N, marks: Option<Marks>) -> ProgramData
where
    N: VisitWith<UsageAnalyzer<ProgramData>>,
{
    analyze_with_storage::<ProgramData, _>(n, marks)
}

/// Analyzed info of a whole program we are working on.
#[derive(Debug, Default)]
pub(crate) struct ProgramData {
    pub(crate) vars: FxHashMap<Id, VarUsageInfo>,

    pub(crate) top: ScopeData,

    pub(crate) scopes: FxHashMap<SyntaxContext, ScopeData>,

    initialized_vars: IndexSet<Id, ARandomState>,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct ScopeData {
    pub(crate) has_with_stmt: bool,
    pub(crate) has_eval_call: bool,
    pub(crate) used_arguments: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct VarUsageInfo {
    pub(crate) inline_prevented: bool,

    /// The number of direct reference to this identifier.
    pub(crate) ref_count: u32,

    /// `false` if it's only used.
    pub(crate) declared: bool,
    pub(crate) declared_count: u32,

    /// `true` if the enclosing function defines this variable as a parameter.
    pub(crate) declared_as_fn_param: bool,

    pub(crate) declared_as_fn_decl: bool,
    pub(crate) declared_as_fn_expr: bool,

    pub(crate) declared_as_for_init: bool,

    /// The number of assign and initialization to this identifier.
    pub(crate) assign_count: u32,

    /// The number of direct and indirect reference to this identifier.
    /// ## Things to note
    ///
    /// - Update is counted as usage, but assign is not
    pub(crate) usage_count: u32,

    /// The variable itself is assigned after reference.
    pub(crate) reassigned: bool,

    pub(crate) has_property_access: bool,
    pub(crate) has_property_mutation: bool,

    pub(crate) exported: bool,
    /// True if used **above** the declaration or in init. (Not eval order).
    pub(crate) used_above_decl: bool,
    /// `true` if it's declared by function parameters or variables declared in
    /// a closest function and used only within it and not used by child
    /// functions.
    pub(crate) is_fn_local: bool,

    used_in_non_child_fn: bool,

    /// `true` if all its assign happens in the same function scope it's defined
    pub(crate) assigned_fn_local: bool,

    pub(crate) executed_multiple_time: bool,
    pub(crate) used_in_cond: bool,

    pub(crate) var_kind: Option<VarDeclKind>,
    pub(crate) var_initialized: bool,

    pub(crate) declared_as_catch_param: bool,

    pub(crate) no_side_effect_for_member_access: bool,

    pub(crate) callee_count: u32,

    /// `a` in `foo(a)` or `foo({ a })`.
    pub(crate) used_as_ref: bool,

    pub(crate) used_as_arg: bool,

    pub(crate) indexed_with_dynamic_key: bool,

    pub(crate) pure_fn: bool,

    /// Is the variable declared in top level?
    pub(crate) is_top_level: bool,

    /// `infects_to`. This should be renamed, but it will be done with another
    /// PR. (because it's hard to review)
    infects_to: Vec<Access>,
    /// Only **string** properties.
    pub(crate) accessed_props: Box<AHashMap<JsWord, u32>>,

    pub(crate) used_recursively: bool,
}

impl Default for VarUsageInfo {
    fn default() -> Self {
        Self {
            inline_prevented: Default::default(),
            ref_count: Default::default(),
            declared: Default::default(),
            declared_count: Default::default(),
            declared_as_fn_param: Default::default(),
            declared_as_fn_decl: Default::default(),
            declared_as_fn_expr: Default::default(),
            declared_as_for_init: Default::default(),
            assign_count: Default::default(),
            usage_count: Default::default(),
            reassigned: Default::default(),
            has_property_access: Default::default(),
            has_property_mutation: Default::default(),
            exported: Default::default(),
            used_above_decl: Default::default(),
            is_fn_local: true,
            executed_multiple_time: Default::default(),
            used_in_cond: Default::default(),
            var_kind: Default::default(),
            var_initialized: Default::default(),
            declared_as_catch_param: Default::default(),
            no_side_effect_for_member_access: Default::default(),
            callee_count: Default::default(),
            used_as_arg: Default::default(),
            indexed_with_dynamic_key: Default::default(),
            pure_fn: Default::default(),
            infects_to: Default::default(),
            used_in_non_child_fn: Default::default(),
            accessed_props: Default::default(),
            used_recursively: Default::default(),
            is_top_level: Default::default(),
            assigned_fn_local: true,
            used_as_ref: false,
        }
    }
}

impl VarUsageInfo {
    pub(crate) fn is_infected(&self) -> bool {
        !self.infects_to.is_empty()
    }

    /// The variable itself or a property of it is modified.
    pub(crate) fn mutated(&self) -> bool {
        self.assign_count > 1 || self.has_property_mutation
    }

    pub(crate) fn can_inline_fn_once(&self) -> bool {
        self.callee_count > 0
            || !self.executed_multiple_time && (self.is_fn_local || !self.used_in_non_child_fn)
    }

    fn initialized(&self) -> bool {
        self.var_initialized || self.declared_as_fn_param || self.declared_as_catch_param
    }
}

impl Storage for ProgramData {
    type ScopeData = ScopeData;
    type VarData = VarUsageInfo;

    fn scope(&mut self, ctxt: swc_common::SyntaxContext) -> &mut Self::ScopeData {
        self.scopes.entry(ctxt).or_default()
    }

    fn top_scope(&mut self) -> &mut Self::ScopeData {
        &mut self.top
    }

    fn var_or_default(&mut self, id: Id) -> &mut Self::VarData {
        self.vars.entry(id).or_default()
    }

    fn merge(&mut self, kind: ScopeKind, child: Self) {
        self.scopes.reserve(child.scopes.len());

        for (ctxt, scope) in child.scopes {
            let to = self.scopes.entry(ctxt).or_default();
            self.top.merge(scope.clone(), true);

            to.merge(scope, false);
        }

        self.vars.reserve(child.vars.len());

        for (id, mut var_info) in child.vars {
            // trace!("merge({:?},{}{:?})", kind, id.0, id.1);
            let inited = self.initialized_vars.contains(&id);
            match self.vars.entry(id.clone()) {
                Entry::Occupied(mut e) => {
                    e.get_mut().inline_prevented |= var_info.inline_prevented;
                    let var_assigned = var_info.assign_count > 0
                        || (var_info.var_initialized && !e.get().var_initialized);

                    if var_info.assign_count > 0 {
                        if e.get().initialized() {
                            e.get_mut().reassigned = true
                        }
                    }

                    if var_info.var_initialized {
                        // If it is inited in some other child scope and also inited in current
                        // scope
                        if e.get().var_initialized || e.get().ref_count > 0 {
                            e.get_mut().reassigned = true;
                        } else {
                            // If it is referred outside child scope, it will
                            // be marked as var_initialized false
                            e.get_mut().var_initialized = true;
                        }
                    } else {
                        // If it is inited in some other child scope, but referenced in
                        // current child scope
                        if !inited && e.get().var_initialized && var_info.ref_count > 0 {
                            e.get_mut().var_initialized = false;
                            e.get_mut().reassigned = true
                        }
                    }

                    e.get_mut().ref_count += var_info.ref_count;

                    e.get_mut().reassigned |= var_info.reassigned;

                    e.get_mut().has_property_access |= var_info.has_property_access;
                    e.get_mut().has_property_mutation |= var_info.has_property_mutation;
                    e.get_mut().exported |= var_info.exported;

                    e.get_mut().declared |= var_info.declared;
                    e.get_mut().declared_count += var_info.declared_count;
                    e.get_mut().declared_as_fn_param |= var_info.declared_as_fn_param;
                    e.get_mut().declared_as_fn_decl |= var_info.declared_as_fn_decl;
                    e.get_mut().declared_as_fn_expr |= var_info.declared_as_fn_expr;
                    e.get_mut().declared_as_catch_param |= var_info.declared_as_catch_param;

                    // If a var is registered at a parent scope, it means that it's delcared before
                    // usages.
                    //
                    // e.get_mut().used_above_decl |= var_info.used_above_decl;
                    e.get_mut().executed_multiple_time |= var_info.executed_multiple_time;
                    e.get_mut().used_in_cond |= var_info.used_in_cond;
                    e.get_mut().assign_count += var_info.assign_count;
                    e.get_mut().usage_count += var_info.usage_count;

                    e.get_mut().infects_to.extend(var_info.infects_to);

                    e.get_mut().no_side_effect_for_member_access =
                        e.get_mut().no_side_effect_for_member_access
                            && var_info.no_side_effect_for_member_access;

                    e.get_mut().callee_count += var_info.callee_count;
                    e.get_mut().used_as_arg |= var_info.used_as_arg;
                    e.get_mut().used_as_ref |= var_info.used_as_ref;
                    e.get_mut().indexed_with_dynamic_key |= var_info.indexed_with_dynamic_key;

                    e.get_mut().pure_fn |= var_info.pure_fn;

                    e.get_mut().used_recursively |= var_info.used_recursively;

                    e.get_mut().is_fn_local &= var_info.is_fn_local;
                    e.get_mut().used_in_non_child_fn |= var_info.used_in_non_child_fn;

                    e.get_mut().assigned_fn_local &= var_info.assigned_fn_local;

                    for (k, v) in *var_info.accessed_props {
                        *e.get_mut().accessed_props.entry(k).or_default() += v;
                    }

                    match kind {
                        ScopeKind::Fn => {
                            e.get_mut().is_fn_local = false;
                            if !var_info.used_recursively {
                                e.get_mut().used_in_non_child_fn = true
                            }

                            if var_assigned {
                                e.get_mut().assigned_fn_local = false
                            }
                        }
                        ScopeKind::Block => {
                            if e.get().used_in_non_child_fn {
                                e.get_mut().is_fn_local = false;
                                e.get_mut().used_in_non_child_fn = true;
                            }
                        }
                    }
                }
                Entry::Vacant(e) => {
                    match kind {
                        ScopeKind::Fn => {
                            if !var_info.used_recursively {
                                var_info.used_in_non_child_fn = true
                            }
                        }
                        ScopeKind::Block => {}
                    }
                    e.insert(var_info);
                }
            }
        }
    }

    fn report_usage(&mut self, ctx: Ctx, i: &Ident, is_assign: bool) {
        self.report(i.to_id(), ctx, is_assign, &mut Default::default());
    }

    fn declare_decl(
        &mut self,
        ctx: Ctx,
        i: &Ident,
        has_init: bool,
        kind: Option<VarDeclKind>,
    ) -> &mut VarUsageInfo {
        // if cfg!(feature = "debug") {
        //     debug!(has_init = has_init, "declare_decl(`{}`)", i);
        // }

        let v = self.vars.entry(i.to_id()).or_default();
        v.is_top_level |= ctx.is_top_level;

        // assigned or declared before this declaration
        if has_init {
            if v.declared || v.var_initialized || v.assign_count > 0 {
                #[cfg(feature = "debug")]
                {
                    tracing::trace!("declare_decl(`{}`): Already declared", i);
                }

                v.reassigned = true;
            }

            v.assign_count += 1;
        }

        // This is not delcared yet, so this is the first declaration.
        if !v.declared {
            v.var_kind = kind;
            v.no_side_effect_for_member_access = ctx.in_decl_with_no_side_effect_for_member_access;
        }

        if v.used_in_non_child_fn {
            v.is_fn_local = false;
        }

        v.var_initialized |= has_init;

        v.declared_count += 1;
        v.declared = true;
        // not a VarDecl, thus always inited
        if has_init || kind.is_none() {
            self.initialized_vars.insert(i.to_id());
        }
        v.declared_as_catch_param |= ctx.in_catch_param;

        v
    }

    fn get_initialized_cnt(&self) -> usize {
        self.initialized_vars.len()
    }

    fn truncate_initialized_cnt(&mut self, len: usize) {
        self.initialized_vars.truncate(len)
    }

    fn mark_property_mutation(&mut self, id: Id, ctx: Ctx) {
        let e = self.vars.entry(id).or_default();
        e.has_property_mutation = true;

        let mut to_mark_mutate = Vec::new();
        for (other, kind) in &e.infects_to {
            if *kind == AccessKind::Reference {
                to_mark_mutate.push(other.clone())
            }
        }

        for other in to_mark_mutate {
            let other = self.vars.entry(other).or_insert_with(|| {
                let simple_assign = ctx.is_exact_assignment && !ctx.is_op_assign;

                VarUsageInfo {
                    used_above_decl: !simple_assign,
                    ..Default::default()
                }
            });

            other.has_property_mutation = true;
        }
    }
}

impl ScopeDataLike for ScopeData {
    fn add_declared_symbol(&mut self, _: &Ident) {}

    fn merge(&mut self, other: Self, _: bool) {
        self.has_with_stmt |= other.has_with_stmt;
        self.has_eval_call |= other.has_eval_call;
        self.used_arguments |= other.used_arguments;
    }

    fn mark_used_arguments(&mut self) {
        self.used_arguments = true;
    }

    fn mark_eval_called(&mut self) {
        self.has_eval_call = true;
    }

    fn mark_with_stmt(&mut self) {
        self.has_with_stmt = true;
    }
}

impl VarDataLike for VarUsageInfo {
    fn mark_declared_as_fn_param(&mut self) {
        self.declared_as_fn_param = true;
    }

    fn mark_declared_as_fn_decl(&mut self) {
        self.declared_as_fn_decl = true;
    }

    fn mark_declared_as_fn_expr(&mut self) {
        self.declared_as_fn_expr = true;
    }

    fn mark_declared_as_for_init(&mut self) {
        self.declared_as_for_init = true;
    }

    fn mark_has_property_access(&mut self) {
        self.has_property_access = true;
    }

    fn mark_used_as_callee(&mut self) {
        self.callee_count += 1;
    }

    fn mark_used_as_arg(&mut self) {
        self.used_as_ref = true;
        self.used_as_arg = true
    }

    fn mark_indexed_with_dynamic_key(&mut self) {
        self.indexed_with_dynamic_key = true;
    }

    fn add_accessed_property(&mut self, name: swc_atoms::JsWord) {
        *self.accessed_props.entry(name).or_default() += 1;
    }

    fn mark_used_as_ref(&mut self) {
        self.used_as_ref = true;
    }

    fn add_infects_to(&mut self, other: Access) {
        self.infects_to.push(other);
    }

    fn prevent_inline(&mut self) {
        self.inline_prevented = true;
    }

    fn mark_as_exported(&mut self) {
        self.exported = true;
    }

    fn mark_initialized_with_safe_value(&mut self) {
        self.no_side_effect_for_member_access = true;
    }

    fn mark_as_pure_fn(&mut self) {
        self.pure_fn = true;
    }

    fn mark_used_above_decl(&mut self) {
        self.used_above_decl = true;
    }

    fn mark_used_recursively(&mut self) {
        self.used_recursively = true;
    }
}

impl ProgramData {
    pub(crate) fn contains_unresolved(&self, e: &Expr) -> bool {
        match e {
            Expr::Ident(i) => {
                if let Some(v) = self.vars.get(&i.to_id()) {
                    return !v.declared;
                }

                true
            }

            Expr::Member(MemberExpr { obj, prop, .. }) => {
                if self.contains_unresolved(obj) {
                    return true;
                }

                if let MemberProp::Computed(prop) = prop {
                    if self.contains_unresolved(&prop.expr) {
                        return true;
                    }
                }

                false
            }

            Expr::Call(CallExpr {
                callee: Callee::Expr(callee),
                args,
                ..
            }) => {
                if self.contains_unresolved(callee) {
                    return true;
                }

                if args.iter().any(|arg| self.contains_unresolved(&arg.expr)) {
                    return true;
                }

                false
            }

            _ => false,
        }
    }
}

impl ProgramData {
    fn report(&mut self, i: Id, ctx: Ctx, is_modify: bool, dejavu: &mut AHashSet<Id>) {
        // trace!("report({}{:?})", i.0, i.1);

        let is_first = dejavu.is_empty();

        if !dejavu.insert(i.clone()) {
            return;
        }

        let inited = self.initialized_vars.contains(&i);

        let e = self.vars.entry(i.clone()).or_insert_with(|| {
            // trace!("insert({}{:?})", i.0, i.1);

            let simple_assign = ctx.is_exact_assignment && !ctx.is_op_assign;

            VarUsageInfo {
                used_above_decl: !simple_assign,
                ..Default::default()
            }
        });

        if is_first {
            e.used_as_ref |= ctx.is_id_ref;
        }

        e.inline_prevented |= ctx.inline_prevented;

        if is_first {
            e.ref_count += 1;
            // If it is inited in some child scope, but referenced in current scope
            if !inited && e.var_initialized {
                e.reassigned = true;
                if !is_modify {
                    e.var_initialized = false;
                    e.assign_count += 1;
                }
            }
        }

        let call_may_mutate = ctx.in_call_arg_of == Some(CalleeKind::Unknown);

        e.executed_multiple_time |= ctx.executed_multiple_time;
        e.used_in_cond |= ctx.in_cond;

        if is_modify && ctx.is_exact_assignment {
            if is_first {
                if e.assign_count > 0 || e.initialized() {
                    e.reassigned = true
                }

                e.assign_count += 1;

                if !ctx.is_op_assign {
                    if e.ref_count == 1 && e.var_kind != Some(VarDeclKind::Const) && !inited {
                        self.initialized_vars.insert(i.clone());
                        e.var_initialized = true;
                    } else {
                        e.reassigned = true
                    }
                }
            }

            if ctx.is_op_assign {
                e.usage_count += 1;
            }

            for other in e.infects_to.clone() {
                self.report(other.0, ctx, true, dejavu)
            }
        } else {
            e.usage_count += 1;
        }

        if call_may_mutate && ctx.is_exact_arg {
            self.mark_property_mutation(i, ctx)
        }
    }
}
