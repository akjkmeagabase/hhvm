(*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is licensed under the MIT license found in the
 * LICENSE file in the "hack" directory of this source tree.
 *
 *)
module Err = Naming_phase_error

(** The derived visitors follows our types. Whenever we have a type
    definition of alias, it will create a corresponding method.

    However, during elaboration / validation, we often want to:
    (1) operate on some _part_ of a type, e.g. a record field
    (2) operate on a list of some type
    (3) distinguish the use of a type in certain positions

    Providing these methods helps to reduce the size of the visitor code (since
    we can recmove all boilerplate) and allows us to put the visitor into
    'normal form' i.e. where we modify either the AST element, the environment,
    generate an error or some combination of the three (this also depends on the
    type of visitor - only mapreduce allows all three).

    We could achieve (2) and (3) by modifying the AST definition, adding more
    type aliases but this doesn't help with (1)

    Here we define custom visitor classes which adds the specific methods we
    need.
*)

class virtual ['self] endo =
  object (self : 'self)
    inherit [_] Aast_defs.endo as super

    method on_'ex _ ex = ex

    method on_'en _ en = en

    (* -- Our elaboration happens at the level of _lists_ of attributes ----- *)
    method on_user_attributes env user_attrs =
      super#on_list self#on_user_attribute env user_attrs

    method on_file_attributes env file_attrs =
      super#on_list self#on_file_attribute env file_attrs

    (* -- Distinguish `context` from `hint` --------------------------------- *)
    method on_context env hint = self#on_hint env hint

    method! on_contexts env (pos, hints) =
      (pos, super#on_list self#on_context env hints)

    (* -- Classes ----------------------------------------------------------- *)
    method on_class_c_tparams env c_tparams =
      super#on_list self#on_tparam env c_tparams

    method on_class_c_extends env c_extends =
      super#on_list self#on_hint env c_extends

    method on_class_c_uses env c_uses = super#on_list self#on_hint env c_uses

    method on_class_c_xhp_attrs env c_xhp_attrs =
      super#on_list self#on_xhp_attr env c_xhp_attrs

    method on_class_c_xhp_attr_uses env c_xhp_attr_uses =
      super#on_list self#on_hint env c_xhp_attr_uses

    method on_class_c_req env (class_hint, require_kind) =
      let class_hint = self#on_class_hint env class_hint
      and require_kind = self#on_require_kind env require_kind in
      (class_hint, require_kind)

    method on_class_c_reqs env c_reqs =
      super#on_list self#on_class_c_req env c_reqs

    method on_class_c_implements env c_implements =
      super#on_list self#on_hint env c_implements

    method on_class_c_where_constraints env c_where_constraints =
      super#on_list self#on_where_constraint_hint env c_where_constraints

    method on_class_c_consts env c_consts =
      super#on_list self#on_class_const env c_consts

    method on_class_c_typeconsts env c_typeconsts =
      super#on_list self#on_class_typeconst_def env c_typeconsts

    method on_class_c_vars env c_vars =
      super#on_list self#on_class_var env c_vars

    method on_class_c_enum env c_enum = super#on_option self#on_enum_ env c_enum

    method on_class_c_methods env c_methods =
      super#on_list self#on_method_ env c_methods

    method on_class_c_user_attributes env c_user_attributes =
      self#on_user_attributes env c_user_attributes

    method on_class_c_file_attributes env c_file_attributes =
      self#on_file_attributes env c_file_attributes

    method! on_class_ env c =
      let c_tparams = self#on_class_c_tparams env c.Aast.c_tparams in

      let c_extends = self#on_class_c_extends env c.Aast.c_extends in

      let c_uses = self#on_class_c_uses env c.Aast.c_uses in

      let c_xhp_attrs = self#on_class_c_xhp_attrs env c.Aast.c_xhp_attrs in

      let c_xhp_attr_uses =
        self#on_class_c_xhp_attr_uses env c.Aast.c_xhp_attr_uses
      in

      let c_reqs = self#on_class_c_reqs env c.Aast.c_reqs in

      let c_implements = self#on_class_c_implements env c.Aast.c_implements in

      let c_where_constraints =
        self#on_class_c_where_constraints env c.Aast.c_where_constraints
      in

      let c_consts = self#on_class_c_consts env c.Aast.c_consts in

      let c_typeconsts = self#on_class_c_typeconsts env c.Aast.c_typeconsts in

      let c_vars = self#on_class_c_vars env c.Aast.c_vars in

      let c_enum = self#on_class_c_enum env c.Aast.c_enum in

      let c_methods = self#on_class_c_methods env c.Aast.c_methods in

      let c_user_attributes =
        self#on_class_c_user_attributes env c.Aast.c_user_attributes
      in

      let c_file_attributes =
        self#on_class_c_file_attributes env c.Aast.c_file_attributes
      in

      Aast.
        {
          c with
          c_tparams;
          c_extends;
          c_uses;
          c_xhp_attr_uses;
          c_reqs;
          c_implements;
          c_where_constraints;
          c_consts;
          c_typeconsts;
          c_vars;
          c_enum;
          c_methods;
          c_xhp_attrs;
          c_user_attributes;
          c_file_attributes;
        }

    (* -- Class vars -------------------------------------------------------- *)

    method! on_class_var env cv =
      let cv_user_attributes =
        self#on_class_var_cv_user_attributes env cv.Aast.cv_user_attributes
      in
      let cv_expr = self#on_class_var_cv_expr env cv.Aast.cv_expr in
      let cv_type = self#on_class_var_cv_type env cv.Aast.cv_type in
      Aast.{ cv with cv_user_attributes; cv_type; cv_expr }

    method on_class_var_cv_user_attributes env cv_user_attributes =
      self#on_user_attributes env cv_user_attributes

    method on_class_var_cv_expr env cv_expr =
      super#on_option self#on_expr env cv_expr

    method on_class_var_cv_type env cv_type = self#on_type_hint env cv_type

    (* -- Functions --------------------------------------------------------- *)
    method! on_fun_ env f =
      let f_ret = self#on_fun_f_ret env f.Aast.f_ret in

      let f_tparams = self#on_fun_f_tparams env f.Aast.f_tparams in

      let f_where_constraints =
        self#on_fun_f_where_constraints env f.Aast.f_where_constraints
      in

      let f_params = self#on_fun_f_params env f.Aast.f_params in

      let f_ctxs = self#on_fun_f_ctxs env f.Aast.f_ctxs in

      let f_unsafe_ctxs = self#on_fun_f_unsafe_ctxs env f.Aast.f_unsafe_ctxs in

      let f_body = self#on_fun_f_body env f.Aast.f_body in

      let f_user_attributes =
        self#on_fun_f_user_attributes env f.Aast.f_user_attributes
      in

      Aast.
        {
          f with
          f_ret;
          f_tparams;
          f_where_constraints;
          f_params;
          f_ctxs;
          f_unsafe_ctxs;
          f_body;
          f_user_attributes;
        }

    method on_fun_f_ret env f_ret = self#on_type_hint env f_ret

    method on_fun_f_tparams env f_tparams =
      super#on_list self#on_tparam env f_tparams

    method on_fun_f_where_constraints env f_where_constraints =
      super#on_list self#on_where_constraint_hint env f_where_constraints

    method on_fun_f_params env f_params =
      super#on_list self#on_fun_param env f_params

    method on_fun_f_ctxs env f_ctxs =
      super#on_option self#on_contexts env f_ctxs

    method on_fun_f_unsafe_ctxs env f_unsafe_ctxs =
      super#on_option self#on_contexts env f_unsafe_ctxs

    method on_fun_f_body env f_body = self#on_func_body env f_body

    method on_fun_f_user_attributes env f_user_attributes =
      self#on_user_attributes env f_user_attributes

    (* -- Methods ----------------------------------------------------------- *)
    method! on_method_ env m =
      let m_tparams = self#on_method_m_tparams env m.Aast.m_tparams in

      let m_where_constraints =
        self#on_method_m_where_constraints env m.Aast.m_where_constraints
      in

      let m_params = self#on_method_m_params env m.Aast.m_params in

      let m_ctxs = self#on_method_m_ctxs env m.Aast.m_ctxs in

      let m_unsafe_ctxs =
        self#on_method_m_unsafe_ctxs env m.Aast.m_unsafe_ctxs
      in

      let m_body = self#on_method_m_body env m.Aast.m_body in

      let m_user_attributes =
        self#on_method_m_user_attributes env m.Aast.m_user_attributes
      in

      let m_ret = self#on_method_m_ret env m.Aast.m_ret in

      Aast.
        {
          m with
          m_tparams;
          m_where_constraints;
          m_params;
          m_ctxs;
          m_unsafe_ctxs;
          m_body;
          m_user_attributes;
          m_ret;
        }

    method on_method_m_ret env f_ret = self#on_type_hint env f_ret

    method on_method_m_tparams env m_tparams =
      super#on_list self#on_tparam env m_tparams

    method on_method_m_where_constraints env m_where_constraints =
      super#on_list self#on_where_constraint_hint env m_where_constraints

    method on_method_m_params env m_params =
      super#on_list self#on_fun_param env m_params

    method on_method_m_ctxs env m_ctxs =
      super#on_option self#on_contexts env m_ctxs

    method on_method_m_unsafe_ctxs env m_unsafe_ctxs =
      super#on_option self#on_contexts env m_unsafe_ctxs

    method on_method_m_body env m_body = self#on_func_body env m_body

    method on_method_m_user_attributes env m_user_attributes =
      self#on_user_attributes env m_user_attributes
    (* -- Hints ------------------------------------------------------------- *)

    method! on_hint_fun env hfun =
      let hf_param_tys =
        self#on_hint_fun_hf_param_tys env hfun.Aast.hf_param_tys
      in
      let hf_variadic_ty =
        self#on_hint_fun_hf_variadic_ty env hfun.Aast.hf_variadic_ty
      in
      let hf_ctxs = self#on_hint_fun_hf_ctxs env hfun.Aast.hf_ctxs in
      let hf_return_ty =
        self#on_hint_fun_hf_return_ty env hfun.Aast.hf_return_ty
      in
      Aast.{ hfun with hf_param_tys; hf_variadic_ty; hf_ctxs; hf_return_ty }

    method on_hint_fun_hf_param_tys env hf_param_tys =
      super#on_list self#on_hint env hf_param_tys

    method on_hint_fun_hf_variadic_ty env hf_variadic_ty =
      super#on_option self#on_hint env hf_variadic_ty

    method on_hint_fun_hf_ctxs env hf_ctxs =
      super#on_option self#on_contexts env hf_ctxs

    method on_hint_fun_hf_return_ty env hf_return_ty =
      self#on_hint env hf_return_ty

    (* -- Global constants -------------------------------------------------- *)

    method! on_gconst env cst =
      let cst_type = self#on_gconst_cst_type env cst.Aast.cst_type in
      let cst_value = self#on_gconst_cst_value env cst.Aast.cst_value in
      Aast.{ cst with cst_type; cst_value }

    method on_gconst_cst_type env cst_type =
      super#on_option self#on_hint env cst_type

    method on_gconst_cst_value env cst_value = self#on_expr env cst_value

    (* -- Helpers ----------------------------------------------------------- *)

    method private on_fst f env (fst, snd) = (f env fst, snd)

    method private on_snd f env (fst, snd) = (fst, f env snd)
  end
