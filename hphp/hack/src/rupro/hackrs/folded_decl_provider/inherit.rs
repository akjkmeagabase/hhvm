// Copyright (c) Facebook, Inc. and its affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the "hack" directory of this source tree.

use super::{subst::Substitution, Result};
use crate::dependency_registrar::{DeclName, DependencyName, DependencyRegistrar};
use indexmap::map::Entry;
use pos::{
    ClassConstNameIndexMap, MethodName, MethodNameIndexMap, Pos, PropNameIndexMap,
    TypeConstNameIndexMap, TypeName, TypeNameIndexMap,
};
use std::sync::Arc;
use ty::decl::{
    folded::Constructor, subst::Subst, ty::ConsistentKind, AbstractTypeconst, Abstraction,
    CeVisibility, ClassConst, ClassConstKind, ClassishKind, DeclTy, FoldedClass, FoldedElement,
    ShallowClass, SubstContext, TypeConst, Typeconst,
};
use ty::reason::Reason;

// note(sf, 2022-02-03): c.f. hphp/hack/src/decl/decl_inherit.ml

#[derive(Debug)]
pub struct Inherited<R: Reason> {
    // note(sf, 2022-01-27): c.f. `Decl_inherit.inherited`
    pub substs: TypeNameIndexMap<SubstContext<R>>,
    pub props: PropNameIndexMap<FoldedElement>,
    pub static_props: PropNameIndexMap<FoldedElement>,
    pub methods: MethodNameIndexMap<FoldedElement>,
    pub static_methods: MethodNameIndexMap<FoldedElement>,
    pub constructor: Constructor,
    pub consts: ClassConstNameIndexMap<ClassConst<R>>,
    pub type_consts: TypeConstNameIndexMap<TypeConst<R>>,
}

impl<R: Reason> Default for Inherited<R> {
    fn default() -> Self {
        Self {
            substs: Default::default(),
            props: Default::default(),
            static_props: Default::default(),
            methods: Default::default(),
            static_methods: Default::default(),
            constructor: Constructor::new(None, ConsistentKind::Inconsistent),
            consts: Default::default(),
            type_consts: Default::default(),
        }
    }
}

impl<R: Reason> Inherited<R> {
    // Reasons to keep the old signature:
    //   - We don't want to override a concrete method with an
    //     abstract one;
    //   - We don't want to override a method that's actually
    //     implemented by the programmer with one that's "synthetic",
    //     e.g. arising merely from a require-extends declaration in a
    //     trait.
    // When these two considerations conflict, we give precedence to
    // abstractness for determining priority of the method.
    fn should_keep_old_sig(new_sig: &FoldedElement, old_sig: &FoldedElement) -> bool {
        !old_sig.is_abstract() && new_sig.is_abstract()
            || old_sig.is_abstract() == new_sig.is_abstract()
                && !old_sig.is_synthesized()
                && new_sig.is_synthesized()
    }

    fn add_constructor(&mut self, constructor: Constructor) {
        let elt = match (constructor.elt.as_ref(), self.constructor.elt.take()) {
            (None, self_ctor) => self_ctor,
            (Some(other_ctor), Some(self_ctor))
                if Self::should_keep_old_sig(other_ctor, &self_ctor) =>
            {
                Some(self_ctor)
            }
            (_, _) => constructor.elt,
        };
        self.constructor = Constructor::new(
            elt,
            ConsistentKind::coalesce(self.constructor.consistency, constructor.consistency),
        );
    }

    fn add_substs(&mut self, other_substs: TypeNameIndexMap<SubstContext<R>>) {
        for (key, new_sc) in other_substs {
            match self.substs.entry(key) {
                Entry::Vacant(e) => {
                    e.insert(new_sc);
                }
                Entry::Occupied(mut e) => {
                    if e.get().from_req_extends && !new_sc.from_req_extends {
                        // If the old substitution context came via required extends
                        // then we want to use the substitutions from the actual
                        // extends instead. e.g.
                        // ```
                        // class Base<+T> {}
                        // trait MyTrait { require extends Base<mixed>; }
                        // class Child extends Base<int> { use MyTrait; }
                        // ```
                        // Here the substitution context `{MyTrait/[T -> mixed]}`
                        // should be overridden by `{Child/[T -> int]}`, because
                        // it's the actual extension of class `Base`.
                        e.insert(new_sc);
                    }
                }
            }
        }
    }

    fn add_method(
        methods: &mut MethodNameIndexMap<FoldedElement>,
        (key, mut fe): (MethodName, FoldedElement),
    ) {
        match methods.entry(key) {
            Entry::Vacant(entry) => {
                // The method didn't exist so far, let's add it.
                entry.insert(fe);
            }
            Entry::Occupied(mut entry) => {
                if !Self::should_keep_old_sig(&fe, entry.get()) {
                    fe.set_is_superfluous_override(false);
                    entry.insert(fe);
                } else {
                    // Otherwise, we *are* overwriting a method
                    // definition. This is OK when a naming
                    // conflict is parent class vs trait (trait
                    // wins!), but not really OK when the naming
                    // conflict is trait vs trait (we rely on HHVM
                    // to catch the error at runtime).
                }
            }
        }
    }

    fn add_methods(&mut self, other_methods: MethodNameIndexMap<FoldedElement>) {
        for (key, fe) in other_methods {
            Self::add_method(&mut self.methods, (key, fe))
        }
    }

    fn add_static_methods(&mut self, other_static_methods: MethodNameIndexMap<FoldedElement>) {
        for (key, fe) in other_static_methods {
            Self::add_method(&mut self.static_methods, (key, fe))
        }
    }

    fn add_props(&mut self, other_props: PropNameIndexMap<FoldedElement>) {
        self.props.extend(other_props)
    }

    fn add_static_props(&mut self, other_static_props: PropNameIndexMap<FoldedElement>) {
        self.static_props.extend(other_static_props)
    }

    fn add_consts(&mut self, other_consts: ClassConstNameIndexMap<ClassConst<R>>) {
        for (name, new_const) in other_consts {
            match self.consts.entry(name) {
                Entry::Vacant(e) => {
                    e.insert(new_const);
                }
                Entry::Occupied(mut e) => {
                    let old_const = e.get();
                    match (
                        new_const.is_synthesized,
                        old_const.is_synthesized,
                        new_const.kind,
                        old_const.kind,
                    ) {
                        (true, false, _, _) => {
                            // Don't replace a constant with a synthesized
                            // constant. This covers the following case:
                            // ```
                            // class HasFoo { abstract const int FOO; }
                            // trait T { require extends Foo; }
                            // class Child extends HasFoo {
                            //   use T;
                            // }
                            // ```
                            // In this case, `Child` still doesn't have a value
                            // for the `FOO` constant.
                        }
                        (_, _, ClassConstKind::CCAbstract(_), ClassConstKind::CCConcrete) => {
                            // Don't replace a concrete constant with an
                            // abstract constant found later in the MRO.
                        }
                        _ => {
                            e.insert(new_const);
                        }
                    }
                }
            }
        }
    }

    fn add_type_consts(&mut self, other_type_consts: TypeConstNameIndexMap<TypeConst<R>>) {
        for (name, mut new_const) in other_type_consts {
            match self.type_consts.entry(name) {
                Entry::Vacant(e) => {
                    // The type constant didn't exist so far, let's add it.
                    e.insert(new_const);
                }
                Entry::Occupied(mut e) => {
                    let old_const = e.get();
                    if new_const.is_enforceable() && !old_const.is_enforceable() {
                        // If some typeconst in some ancestor was enforceable,
                        // then the child class' typeconst will be enforceable
                        // too, even if we didn't take that ancestor typeconst.
                        e.get_mut().enforceable = new_const.enforceable.clone();
                    }
                    let old_const = e.get();
                    match (&old_const.kind, &new_const.kind) {
                        // This covers the following case
                        // ```
                        // interface I1 { abstract const type T; }
                        // interface I2 { const type T = int; }
                        // class C implements I1, I2 {}
                        // ```
                        // Then `C::T == I2::T` since `I2::T `is not abstract
                        (Typeconst::TCConcrete(_), Typeconst::TCAbstract(_)) => {}
                        // This covers the following case
                        // ```
                        // interface I {
                        //   abstract const type T as arraykey;
                        // }
                        //
                        // abstract class A {
                        //   abstract const type T as arraykey = string;
                        // }
                        //
                        // final class C extends A implements I {}
                        // ```
                        // `C::T` must come from `A`, not `I`, as `A`
                        // provides the default that will synthesize into a
                        // concrete type constant in `C`.
                        (
                            Typeconst::TCAbstract(AbstractTypeconst {
                                default: Some(_), ..
                            }),
                            Typeconst::TCAbstract(AbstractTypeconst { default: None, .. }),
                        ) => {}
                        // When a type constant is declared in multiple
                        // parents we need to make a subtle choice of what
                        // type we inherit. For example in:
                        // ```
                        // interface I1 { abstract const type t as Container<int>; }
                        // interface I2 { abstract const type t as KeyedContainer<int, int>; }
                        // abstract class C implements I1, I2 {}
                        // ```
                        // Depending on the order the interfaces are
                        // declared, we may report an error. Since this
                        // could be confusing there is special logic in
                        // `Typing_extends` that checks for this potentially
                        // ambiguous situation and warns the programmer to
                        // explicitly declare `T` in `C`.
                        _ => {
                            if old_const.is_enforceable() && !new_const.is_enforceable() {
                                // If a typeconst we already inherited from some
                                // other ancestor was enforceable, then the one
                                // we inherit here will be enforceable too.
                                new_const.enforceable = old_const.enforceable.clone();
                            }
                            e.insert(new_const);
                        }
                    }
                }
            }
        }
    }

    fn add_inherited(&mut self, other: Self) {
        let Self {
            substs,
            props,
            static_props,
            methods,
            static_methods,
            constructor,
            consts,
            type_consts,
        } = other;
        self.add_substs(substs);
        self.add_props(props);
        self.add_static_props(static_props);
        self.add_methods(methods);
        self.add_static_methods(static_methods);
        self.add_constructor(constructor);
        self.add_consts(consts);
        self.add_type_consts(type_consts);
    }

    fn mark_as_synthesized(&mut self) {
        (self.substs.values_mut()).for_each(|s| s.set_from_req_extends(true));
        (self.constructor.elt.iter_mut()).for_each(|e| e.set_is_synthesized(true));
        (self.props.values_mut()).for_each(|p| p.set_is_synthesized(true));
        (self.static_props.values_mut()).for_each(|p| p.set_is_synthesized(true));
        (self.methods.values_mut()).for_each(|m| m.set_is_synthesized(true));
        (self.static_methods.values_mut()).for_each(|m| m.set_is_synthesized(true));
        (self.consts.values_mut()).for_each(|c| c.set_is_synthesized(true));
        (self.type_consts.values_mut()).for_each(|c| c.set_is_synthesized(true));
    }
}

struct MemberFolder<'a, R: Reason> {
    child: &'a ShallowClass<R>,
    parents: &'a TypeNameIndexMap<Arc<FoldedClass<R>>>,
    dependency_registrar: &'a dyn DependencyRegistrar,
    members: Inherited<R>,
}

impl<'a, R: Reason> MemberFolder<'a, R> {
    // c.f. `Decl_inherit.from_class` and `Decl_inherit.inherit_hack_class`.
    fn members_from_class(&self, parent_ty: &DeclTy<R>) -> Result<Inherited<R>> {
        fn is_not_private<N>((_, elt): &(&N, &FoldedElement)) -> bool {
            match elt.visibility {
                CeVisibility::Private(_) if elt.is_lsb() => true,
                CeVisibility::Private(_) => false,
                _ => true,
            }
        }
        fn chown(elt: FoldedElement, owner: TypeName) -> FoldedElement {
            match elt.visibility {
                CeVisibility::Private(_) => FoldedElement {
                    visibility: CeVisibility::Private(owner),
                    ..elt
                },
                CeVisibility::Protected(_) if !elt.is_synthesized() => FoldedElement {
                    visibility: CeVisibility::Protected(owner),
                    ..elt
                },
                _ => elt,
            }
        }

        if let Some((_, parent_pos_id, parent_tyl)) = parent_ty.unwrap_class_type() {
            if let Some(parent_folded_decl) = self.parents.get(&parent_pos_id.id()) {
                let sig = Subst::new(&parent_folded_decl.tparams, parent_tyl);
                let subst = Substitution { subst: &sig };

                let consts = (parent_folded_decl.consts.iter())
                    .map(|(name, cc)| (*name, subst.instantiate_class_const(cc)))
                    .collect();
                let type_consts = (parent_folded_decl.type_consts.iter())
                    .map(|(name, tc)| (*name, subst.instantiate_type_const(tc)))
                    .collect();

                let parent_inh = match parent_folded_decl.kind {
                    ClassishKind::Ctrait => Inherited {
                        consts,
                        type_consts,
                        props: (parent_folded_decl.props.iter())
                            .map(|(k, v)| (*k, chown(v.clone(), self.child.name.id())))
                            .collect(),
                        static_props: (parent_folded_decl.static_props.iter())
                            .map(|(k, v)| (*k, chown(v.clone(), self.child.name.id())))
                            .collect(),
                        methods: (parent_folded_decl.methods)
                            .iter()
                            .map(|(k, v)| (*k, chown(v.clone(), self.child.name.id())))
                            .collect(),
                        static_methods: (parent_folded_decl.static_methods.iter())
                            .map(|(k, v)| (*k, chown(v.clone(), self.child.name.id())))
                            .collect(),
                        ..Default::default()
                    },
                    ClassishKind::Cclass(_) | ClassishKind::Cinterface => Inherited {
                        consts,
                        type_consts,
                        props: (parent_folded_decl.props.iter())
                            .filter(is_not_private)
                            .map(|(k, v)| (*k, v.clone()))
                            .collect(),
                        static_props: (parent_folded_decl.static_props.iter())
                            .filter(is_not_private)
                            .map(|(k, v)| (*k, v.clone()))
                            .collect(),
                        methods: (parent_folded_decl.methods.iter())
                            .filter(is_not_private)
                            .map(|(k, v)| (*k, v.clone()))
                            .collect(),
                        static_methods: (parent_folded_decl.static_methods.iter())
                            .filter(is_not_private)
                            .map(|(k, v)| (*k, v.clone()))
                            .collect(),
                        ..Default::default()
                    },
                    ClassishKind::Cenum | ClassishKind::CenumClass(_) => Inherited {
                        consts,
                        type_consts,
                        props: parent_folded_decl.props.clone(),
                        static_props: parent_folded_decl.static_props.clone(),
                        methods: parent_folded_decl.methods.clone(),
                        static_methods: parent_folded_decl.static_methods.clone(),
                        ..Default::default()
                    },
                };

                // TODO(hrust): Do we need sharing?
                let mut substs = parent_folded_decl.substs.clone();
                substs.insert(
                    parent_folded_decl.name,
                    SubstContext {
                        subst: sig,
                        class_context: self.child.name.id(),
                        from_req_extends: false,
                    },
                );

                let constructor = parent_folded_decl.constructor.clone();
                if !parent_folded_decl.pos.is_hhi() {
                    self.dependency_registrar.add_dependency(
                        DeclName::Type(self.child.name.id()),
                        DependencyName::Constructor(parent_folded_decl.name),
                    )?;
                }

                return Ok(Inherited {
                    substs,
                    constructor,
                    ..parent_inh
                });
            }
        }

        Ok(Default::default())
    }

    fn class_constants_from_class(&self, ty: &DeclTy<R>) -> Result<Inherited<R>> {
        if let Some((_, pos_id, tyl)) = ty.unwrap_class_type() {
            if let Some(class) = self.parents.get(&pos_id.id()) {
                let sig = Subst::new(&class.tparams, tyl);
                let subst = Substitution { subst: &sig };
                let consts: ClassConstNameIndexMap<_> = class
                    .consts
                    .iter()
                    .map(|(name, cc)| (*name, subst.instantiate_class_const(cc)))
                    .collect();
                let type_consts: TypeConstNameIndexMap<_> = class
                    .type_consts
                    .iter()
                    .map(|(name, tc)| (*name, subst.instantiate_type_const(tc)))
                    .collect();
                return Ok(Inherited {
                    consts,
                    type_consts,
                    ..Default::default()
                });
            }
        }

        Ok(Default::default())
    }

    // This logic deals with importing XHP attributes from an XHP class via the
    // "attribute :foo" syntax.
    // c.f. Decl_inherit.from_class_xhp_attrs_only
    fn xhp_attrs_from_class(&self, ty: &DeclTy<R>) -> Result<Inherited<R>> {
        if let Some((_, pos_id, _tyl)) = ty.unwrap_class_type() {
            if let Some(class) = self.parents.get(&pos_id.id()) {
                // Filter out properties that are not XHP attributes.
                return Ok(Inherited {
                    props: class
                        .props
                        .iter()
                        .filter(|(_, prop)| prop.get_xhp_attr().is_some())
                        .map(|(name, prop)| (name.clone(), prop.clone()))
                        .collect(),
                    ..Default::default()
                });
            }
        }

        Ok(Default::default())
    }

    fn add_from_interface_constants(&mut self) -> Result<()> {
        for ty in self.child.req_implements.iter() {
            self.members
                .add_inherited(self.class_constants_from_class(ty)?)
        }

        Ok(())
    }

    fn add_from_implements_constants(&mut self) -> Result<()> {
        for ty in self.child.implements.iter() {
            self.members
                .add_inherited(self.class_constants_from_class(ty)?)
        }

        Ok(())
    }

    fn add_from_xhp_attr_uses(&mut self) -> Result<()> {
        for ty in self.child.xhp_attr_uses.iter() {
            self.members.add_inherited(self.xhp_attrs_from_class(ty)?)
        }

        Ok(())
    }

    fn add_from_parents(&mut self) -> Result<()> {
        let mut tys: Vec<&DeclTy<R>> = Vec::new();
        match self.child.kind {
            ClassishKind::Cclass(Abstraction::Abstract) => {
                tys.extend(self.child.implements.iter());
                tys.extend(self.child.extends.iter());
            }
            ClassishKind::Ctrait => {
                tys.extend(self.child.implements.iter());
                tys.extend(self.child.extends.iter());
                tys.extend(self.child.req_implements.iter());
            }
            ClassishKind::Cclass(_)
            | ClassishKind::Cinterface
            | ClassishKind::Cenum
            | ClassishKind::CenumClass(_) => {
                tys.extend(self.child.extends.iter());
            }
        };

        // Interfaces implemented, classes extended and interfaces required to
        // be implemented.
        for ty in tys.iter().rev() {
            self.members.add_inherited(self.members_from_class(ty)?);
        }

        Ok(())
    }

    fn add_from_requirements(&mut self) -> Result<()> {
        for ty in self.child.req_extends.iter() {
            let mut inherited = self.members_from_class(ty)?;
            inherited.mark_as_synthesized();
            self.members.add_inherited(inherited);
        }

        Ok(())
    }

    fn add_from_traits(&mut self) -> Result<()> {
        for ty in self.child.uses.iter() {
            self.members.add_inherited(self.members_from_class(ty)?);
        }

        Ok(())
    }

    fn add_from_included_enums_constants(&mut self) -> Result<()> {
        if let Some(et) = self.child.enum_type.as_ref() {
            for ty in et.includes.iter() {
                self.members
                    .add_inherited(self.class_constants_from_class(ty)?);
            }
        }

        Ok(())
    }
}

impl<R: Reason> Inherited<R> {
    pub fn make(
        child: &ShallowClass<R>,
        parents: &TypeNameIndexMap<Arc<FoldedClass<R>>>,
        dependency_registrar: &dyn DependencyRegistrar,
    ) -> Result<Self> {
        let mut folder = MemberFolder {
            child,
            parents,
            dependency_registrar,
            members: Self::default(),
        };
        folder.add_from_parents()?; // Members inherited from parents ...
        folder.add_from_requirements()?;
        folder.add_from_traits()?; // ... can be overridden by traits.
        folder.add_from_xhp_attr_uses()?;
        folder.add_from_interface_constants()?;
        folder.add_from_included_enums_constants()?;
        folder.add_from_implements_constants()?;

        Ok(folder.members)
    }
}