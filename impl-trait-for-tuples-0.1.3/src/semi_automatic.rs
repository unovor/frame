//! Implementation of the semi-automatic tuple trait implementation.
//!
//! The semi-automatic implementation uses an implementation provided by the user to generate the
//! tuple implementations. The user is able to use a special syntax `for_tuples!( #(TUPLE)* );` to
//! express the tuple access while the `TUPLE` ident can be chosen by the user.

use proc_macro2::TokenStream;

use syn::{
    bracketed,
    fold::{self, Fold},
    parenthesized,
    parse::{Parse, ParseStream},
    parse_quote,
    spanned::Spanned,
    token, Block, Error, Expr, ExprField, FnArg, Ident, ImplItem, ImplItemMethod, Index, ItemImpl,
    Lifetime, Macro, Member, Result, Stmt, Type, WhereClause, WherePredicate,
};

use quote::{quote, ToTokens};

/// By default we add the trait bound for the implemented trait to each tuple type. When this
/// attribute is given we don't add this bound.
const TUPLE_TYPES_NO_DEFAULT_TRAIT_BOUND: &str = "tuple_types_no_default_trait_bound";

/// The supported separators in the `#( Tuple::test() )SEPARATOR*` syntax.
enum Separator {
    Comma(token::Comma),
    Add(token::Add),
    Sub(token::Sub),
    Or(token::Or),
    And(token::And),
    Star(token::Star),
    Div(token::Div),
}

impl Separator {
    /// Try to parse the separator before the `*` token.
    fn parse_before_star(input: ParseStream) -> Result<Option<Self>> {
        if input.peek2(token::Star) {
            Self::parse(input).map(Some)
        } else {
            Ok(None)
        }
    }

    /// Convert into a `TokenStream`.
    ///
    /// `last` - Is this the last separator to add? Only `,` will be added on `last == true`.
    fn to_token_stream(&self, last: bool) -> TokenStream {
        let empty_on_last = |token: &dyn ToTokens| {
            if last {
                TokenStream::default()
            } else {
                token.to_token_stream()
            }
        };

        match self {
            Self::Comma(comma) => comma.to_token_stream(),
            Self::Add(add) => empty_on_last(add),
            Self::Sub(sub) => empty_on_last(sub),
            Self::Or(or) => empty_on_last(or),
            Self::And(and) => empty_on_last(and),
            Self::Star(star) => empty_on_last(star),
            Self::Div(div) => empty_on_last(div),
        }
    }
}

impl Parse for Separator {
    fn parse(input: ParseStream) -> Result<Self> {
        let lookahead1 = input.lookahead1();

        if lookahead1.peek(token::Comma) {
            Ok(Self::Comma(input.parse()?))
        } else if lookahead1.peek(token::Add) {
            Ok(Self::Add(input.parse()?))
        } else if lookahead1.peek(token::Sub) {
            Ok(Self::Sub(input.parse()?))
        } else if lookahead1.peek(token::Or) {
            Ok(Self::Or(input.parse()?))
        } else if lookahead1.peek(token::And) {
            Ok(Self::And(input.parse()?))
        } else if lookahead1.peek(token::Star) {
            Ok(Self::Star(input.parse()?))
        } else if lookahead1.peek(token::Div) {
            Ok(Self::Div(input.parse()?))
        } else {
            Err(lookahead1.error())
        }
    }
}

/// The `#( Tuple::test() )SEPARATOR*` (tuple repetition) syntax.
struct TupleRepetition {
    pub pound_token: token::Pound,
    pub paren_token: token::Paren,
    pub stmts: Vec<Stmt>,
    pub where_predicate: Option<WherePredicate>,
    pub separator: Option<Separator>,
    pub star_token: token::Star,
}

impl TupleRepetition {
    /// Parse the inner representation as stmts.
    fn parse_as_stmts(input: ParseStream) -> Result<Self> {
        let content;
        Ok(Self {
            pound_token: input.parse()?,
            paren_token: parenthesized!(content in input),
            stmts: content.call(Block::parse_within)?,
            where_predicate: None,
            separator: Separator::parse_before_star(input)?,
            star_token: input.parse()?,
        })
    }

    /// Parse the inner representation as a where predicate.
    fn parse_as_where_predicate(input: ParseStream) -> Result<Self> {
        let content;
        Ok(Self {
            pound_token: input.parse()?,
            paren_token: parenthesized!(content in input),
            stmts: Vec::new(),
            where_predicate: Some(content.parse()?),
            separator: Separator::parse_before_star(input)?,
            star_token: input.parse()?,
        })
    }

    /// Expand this repetition to the actual stmts implementation.
    fn expand_as_stmts(
        self,
        tuple_placeholder_ident: &Ident,
        tuples: &[Ident],
        use_self: bool,
    ) -> TokenStream {
        let mut generated = TokenStream::new();

        for (i, tuple) in tuples.iter().enumerate() {
            generated.extend(self.stmts.iter().cloned().map(|s| {
                ReplaceTuplePlaceholder::replace_ident_in_stmt(
                    tuple_placeholder_ident,
                    tuple,
                    use_self,
                    i,
                    s,
                )
                .map(|s| s.to_token_stream())
                .unwrap_or_else(|e| e.to_compile_error())
            }));

            if let Some(ref sep) = self.separator {
                generated.extend(sep.to_token_stream(i + 1 == tuples.len()));
            }
        }

        generated
    }

    /// Expand this to the given `where_clause`.
    /// It is expected that the instance was created with `parse_as_where_predicate`.
    fn expand_to_where_clause(
        self,
        tuple_placeholder_ident: &Ident,
        tuples: &[Ident],
        where_clause: &mut WhereClause,
    ) -> Result<()> {
        let span = self.pound_token.span();
        let predicate = self.where_predicate.ok_or_else(|| {
            Error::new(
                span,
                "Internal error, expected `where_predicate` to be set! Please report this issue!",
            )
        })?;

        for tuple in tuples.iter() {
            where_clause.predicates.push(
                ReplaceTuplePlaceholder::replace_ident_in_where_predicate(
                    tuple_placeholder_ident,
                    tuple,
                    predicate.clone(),
                )?,
            );
        }

        Ok(())
    }
}

/// Replace the tuple place holder in the ast.
struct ReplaceTuplePlaceholder<'a> {
    search: &'a Ident,
    replace: &'a Ident,
    use_self: bool,
    index: Index,
    errors: Vec<Error>,
}

impl<'a> ReplaceTuplePlaceholder<'a> {
    fn replace_ident_in_stmt(
        search: &'a Ident,
        replace: &'a Ident,
        use_self: bool,
        index: usize,
        stmt: Stmt,
    ) -> Result<Stmt> {
        let mut folder = Self {
            search,
            replace,
            use_self,
            index: index.into(),
            errors: Vec::new(),
        };

        let res = fold::fold_stmt(&mut folder, stmt);

        if let Some(first) = folder.errors.pop() {
            Err(folder.errors.into_iter().fold(first, |mut e, n| {
                e.combine(n);
                e
            }))
        } else {
            Ok(res)
        }
    }

    fn replace_ident_in_where_predicate(
        search: &'a Ident,
        replace: &'a Ident,
        where_predicate: WherePredicate,
    ) -> Result<WherePredicate> {
        let mut folder = Self {
            search,
            replace,
            use_self: false,
            index: 0.into(),
            errors: Vec::new(),
        };

        let res = fold::fold_where_predicate(&mut folder, where_predicate);

        if let Some(first) = folder.errors.pop() {
            Err(folder.errors.into_iter().fold(first, |mut e, n| {
                e.combine(n);
                e
            }))
        } else {
            Ok(res)
        }
    }
}

impl<'a> Fold for ReplaceTuplePlaceholder<'a> {
    fn fold_ident(&mut self, ident: Ident) -> Ident {
        if &ident == self.search {
            self.replace.clone()
        } else {
            ident
        }
    }

    fn fold_expr(&mut self, expr: Expr) -> Expr {
        match expr {
            Expr::MethodCall(mut call) => match *call.receiver {
                Expr::Path(ref path) if path.path.is_ident(self.search) => {
                    if self.use_self {
                        let index = &self.index;
                        call.receiver = parse_quote!( self.#index );

                        call.into()
                    } else {
                        self.errors.push(Error::new(
                            path.span(),
                            "Can not call non-static method from within a static method.",
                        ));
                        Expr::Verbatim(Default::default())
                    }
                }
                _ => fold::fold_expr_method_call(self, call).into(),
            },
            _ => fold::fold_expr(self, expr),
        }
    }

    fn fold_expr_field(&mut self, mut expr: ExprField) -> ExprField {
        match expr.member {
            Member::Named(ref ident) if ident == self.search => {
                // Replace `something.Tuple` with `something.0`, `something.1`, etc.
                expr.member = Member::Unnamed(self.index.clone());
                expr
            }
            _ => expr,
        }
    }
}

/// The `for_tuples!` macro syntax.
enum ForTuplesMacro {
    /// The macro at an item type position.
    ///
    /// `type Item =`
    ItemType {
        type_token: token::Type,
        ident: Ident,
        equal_token: token::Eq,
        paren_token: token::Paren,
        tuple_repetition: TupleRepetition,
        semi_token: token::Semi,
    },
    /// The macro at an item const position.
    ///
    /// `const Item: Type =`
    ItemConst {
        const_token: token::Const,
        ident: Ident,
        colon_token: token::Colon,
        ty: Type,
        equal_token: token::Eq,
        /// When this is a `slice` type, we need to parse `&[]`.
        slice: Option<(token::And, token::Bracket)>,
        tuple_repetition: TupleRepetition,
        semi_token: token::Semi,
    },
    /// The repetition stmt wrapped in parenthesis.
    StmtParenthesized {
        paren_token: token::Paren,
        tuple_repetition: TupleRepetition,
    },
    /// Just the repetition stmt.
    Stmt { tuple_repetition: TupleRepetition },
    /// A custom where clause.
    Where {
        _where_token: token::Where,
        tuple_repetition: TupleRepetition,
    },
}

impl Parse for ForTuplesMacro {
    fn parse(input: ParseStream) -> Result<Self> {
        let lookahead1 = input.lookahead1();

        if lookahead1.peek(token::Type) {
            let content;
            Ok(ForTuplesMacro::ItemType {
                type_token: input.parse()?,
                ident: input.parse()?,
                equal_token: input.parse()?,
                paren_token: parenthesized!(content in input),
                tuple_repetition: content.call(TupleRepetition::parse_as_stmts)?,
                semi_token: input.parse()?,
            })
        } else if lookahead1.peek(token::Const) {
            let const_token = input.parse()?;
            let ident = input.parse()?;
            let colon_token = input.parse()?;
            let ty = input.parse()?;
            let equal_token = input.parse()?;

            let lookahead1 = input.lookahead1();

            if lookahead1.peek(token::And) {
                let content;

                Ok(ForTuplesMacro::ItemConst {
                    const_token,
                    ident,
                    colon_token,
                    ty,
                    equal_token,
                    slice: Some((input.parse()?, bracketed!(content in input))),
                    tuple_repetition: content.call(TupleRepetition::parse_as_stmts)?,
                    semi_token: input.parse()?,
                })
            } else if lookahead1.peek(token::Pound) {
                Ok(ForTuplesMacro::ItemConst {
                    const_token,
                    ident,
                    colon_token,
                    ty,
                    equal_token,
                    slice: None,
                    tuple_repetition: input.call(TupleRepetition::parse_as_stmts)?,
                    semi_token: input.parse()?,
                })
            } else {
                Err(lookahead1.error())
            }
        } else if lookahead1.peek(token::Paren) {
            let content;
            Ok(ForTuplesMacro::StmtParenthesized {
                paren_token: parenthesized!(content in input),
                tuple_repetition: content.call(TupleRepetition::parse_as_stmts)?,
            })
        } else if lookahead1.peek(token::Pound) {
            Ok(ForTuplesMacro::Stmt {
                tuple_repetition: input.call(TupleRepetition::parse_as_stmts)?,
            })
        } else if lookahead1.peek(token::Where) {
            Ok(ForTuplesMacro::Where {
                _where_token: input.parse()?,
                tuple_repetition: input.call(TupleRepetition::parse_as_where_predicate)?,
            })
        } else {
            Err(lookahead1.error())
        }
    }
}

impl ForTuplesMacro {
    /// Try to parse the given macro as `Self`.
    ///
    /// `allow_where` signals that a custom where clause is allowed at this position.
    ///
    /// Returns `Ok(None)` if it is not a `for_tuples!` macro.
    fn try_from(macro_item: &Macro, allow_where: bool) -> Result<Option<Self>> {
        // Not the macro we are searching for
        if !macro_item.path.is_ident("for_tuples") {
            return Ok(None);
        }

        let res = macro_item.parse_body::<Self>()?;

        if !allow_where && res.is_where() {
            Err(Error::new(
                macro_item.span(),
                "Custom where clause not allowed at this position!",
            ))
        } else {
            Ok(Some(res))
        }
    }

    /// Is this a custom where clause?
    fn is_where(&self) -> bool {
        match self {
            Self::Where { .. } => true,
            _ => false,
        }
    }

    /// Convert this into the where clause tuple repetition.
    fn into_where(self) -> Option<TupleRepetition> {
        match self {
            Self::Where {
                tuple_repetition, ..
            } => Some(tuple_repetition),
            _ => None,
        }
    }

    /// Expand `self` to the actual implementation without the `for_tuples!` macro.
    ///
    /// This will unroll the repetition by replacing the placeholder identifier in each iteration
    /// with the one given in `tuples`. If `use_self` is `true`, the tuple will be access by using
    /// `self.x`.
    ///
    /// Returns the generated code.
    fn expand(
        self,
        tuple_placeholder_ident: &Ident,
        tuples: &[Ident],
        use_self: bool,
    ) -> TokenStream {
        match self {
            Self::ItemType {
                type_token,
                ident,
                equal_token,
                paren_token,
                tuple_repetition,
                semi_token,
            } => {
                let mut token_stream = type_token.to_token_stream();
                let repetition =
                    tuple_repetition.expand_as_stmts(tuple_placeholder_ident, tuples, use_self);

                ident.to_tokens(&mut token_stream);
                equal_token.to_tokens(&mut token_stream);
                paren_token.surround(&mut token_stream, |tokens| tokens.extend(repetition));
                semi_token.to_tokens(&mut token_stream);

                token_stream
            }
            Self::ItemConst {
                const_token,
                ident,
                colon_token,
                ty,
                equal_token,
                slice,
                tuple_repetition,
                semi_token,
            } => {
                let mut token_stream = const_token.to_token_stream();
                let repetition =
                    tuple_repetition.expand_as_stmts(tuple_placeholder_ident, tuples, use_self);

                ident.to_tokens(&mut token_stream);
                colon_token.to_tokens(&mut token_stream);
                ty.to_tokens(&mut token_stream);
                equal_token.to_tokens(&mut token_stream);
                match slice {
                    Some((ref_token, brackets)) => {
                        ref_token.to_tokens(&mut token_stream);
                        brackets.surround(&mut token_stream, |tokens| tokens.extend(repetition));
                    }
                    None => token_stream.extend(repetition),
                }
                semi_token.to_tokens(&mut token_stream);

                token_stream
            }
            Self::StmtParenthesized {
                paren_token,
                tuple_repetition,
            } => {
                let mut token_stream = TokenStream::new();
                let repetition =
                    tuple_repetition.expand_as_stmts(tuple_placeholder_ident, tuples, use_self);

                paren_token.surround(&mut token_stream, |tokens| tokens.extend(repetition));

                token_stream
            }
            Self::Stmt { tuple_repetition } => {
                tuple_repetition.expand_as_stmts(tuple_placeholder_ident, tuples, use_self)
            }
            Self::Where { .. } => TokenStream::new(),
        }
    }
}

/// Add the tuple elements as generic parameters to the given trait implementation.
fn add_tuple_elements_generics(
    tuples: &[Ident],
    mut trait_impl: ItemImpl,
    add_bound: bool,
) -> Result<ItemImpl> {
    let trait_ = trait_impl.trait_.clone().map(|t| t.1).ok_or_else(|| {
        Error::new(
            trait_impl.span(),
            "The semi-automatic implementation is required to implement a trait!",
        )
    })?;

    let bound = if add_bound {
        Some(quote!( #trait_ ))
    } else {
        None
    };
    crate::utils::add_tuple_element_generics(tuples, bound, &mut trait_impl.generics);
    Ok(trait_impl)
}

/// Fold a given trait implementation into a tuple implementation of the given trait.
struct ToTupleImplementation<'a> {
    /// The tuple idents to use while expanding the repetitions.
    tuples: &'a [Ident],
    /// The placeholder ident given by the user.
    ///
    /// This placeholder ident while be replaced in the expansion with the correct tuple identifiers.
    tuple_placeholder_ident: &'a Ident,
    /// Any errors found while doing the conversion.
    errors: Vec<Error>,
    /// This is set to `true`, when folding in a function block that has a `self` parameter.
    has_self_parameter: bool,
    /// A custom where clause provided by the user.
    custom_where_clause: Option<TupleRepetition>,
}

impl<'a> ToTupleImplementation<'a> {
    /// Generate the tuple implementation for the given `tuples`.
    fn generate_implementation(
        trait_impl: &ItemImpl,
        tuple_placeholder_ident: &'a Ident,
        tuples: &'a [Ident],
    ) -> Result<TokenStream> {
        let mut to_tuple = ToTupleImplementation {
            tuples,
            errors: Vec::new(),
            tuple_placeholder_ident,
            has_self_parameter: false,
            custom_where_clause: None,
        };

        let mut res = fold::fold_item_impl(&mut to_tuple, trait_impl.clone());

        // Check if we should add the bound to the implemented trait for each tuple type.
        let add_bound = if let Some(pos) = res
            .attrs
            .iter()
            .position(|a| a.path.is_ident(TUPLE_TYPES_NO_DEFAULT_TRAIT_BOUND))
        {
            res.attrs.remove(pos);
            false
        } else {
            true
        };

        // Add the tuple generics
        let mut res = add_tuple_elements_generics(tuples, res, add_bound)?;
        // Add the correct self type
        res.self_ty = parse_quote!( ( #( #tuples, )* ) );
        res.attrs.push(parse_quote!(#[allow(unused)]));

        if let Some(where_clause) = to_tuple.custom_where_clause.take() {
            where_clause.expand_to_where_clause(
                tuple_placeholder_ident,
                tuples,
                res.generics.make_where_clause(),
            )?;
        }

        if let Some(first_error) = to_tuple.errors.pop() {
            Err(to_tuple.errors.into_iter().fold(first_error, |mut e, n| {
                e.combine(n);
                e
            }))
        } else {
            Ok(res.to_token_stream())
        }
    }

    /// Fold the expr and returns the folded expr and if it was a `for_tuples!`.
    fn custom_fold_expr(&mut self, expr: Expr) -> (Expr, bool) {
        match expr {
            Expr::Macro(expr_macro) => match ForTuplesMacro::try_from(&expr_macro.mac, false) {
                Ok(Some(for_tuples)) => (
                    Expr::Verbatim(for_tuples.expand(
                        &self.tuple_placeholder_ident,
                        self.tuples,
                        self.has_self_parameter,
                    )),
                    true,
                ),
                Ok(None) => (fold::fold_expr_macro(self, expr_macro).into(), false),
                Err(e) => {
                    self.errors.push(e);
                    (Expr::Verbatim(Default::default()), false)
                }
            },
            _ => (fold::fold_expr(self, expr), false),
        }
    }
}

impl<'a> Fold for ToTupleImplementation<'a> {
    fn fold_impl_item(&mut self, i: ImplItem) -> ImplItem {
        match i {
            ImplItem::Macro(macro_item) => match ForTuplesMacro::try_from(&macro_item.mac, true) {
                Ok(Some(for_tuples)) => {
                    if for_tuples.is_where() {
                        if self.custom_where_clause.is_some() {
                            self.errors.push(Error::new(
                                macro_item.span(),
                                "Only one custom where clause is supported!",
                            ));
                        } else {
                            self.custom_where_clause = for_tuples.into_where();
                        }

                        ImplItem::Verbatim(Default::default())
                    } else {
                        ImplItem::Verbatim(for_tuples.expand(
                            &self.tuple_placeholder_ident,
                            self.tuples,
                            false,
                        ))
                    }
                }
                Ok(None) => fold::fold_impl_item_macro(self, macro_item).into(),
                Err(e) => {
                    self.errors.push(e);
                    ImplItem::Verbatim(Default::default())
                }
            },
            _ => fold::fold_impl_item(self, i),
        }
    }

    fn fold_expr(&mut self, expr: Expr) -> Expr {
        self.custom_fold_expr(expr).0
    }

    fn fold_stmt(&mut self, stmt: Stmt) -> Stmt {
        let (expr, trailing_semi) = match stmt {
            Stmt::Expr(expr) => (expr, None),
            Stmt::Semi(expr, semi) => (expr, Some(semi)),
            _ => return fold::fold_stmt(self, stmt),
        };

        let (expr, expanded) = self.custom_fold_expr(expr);

        if expanded {
            Stmt::Expr(expr)
        } else if let Some(semi) = trailing_semi {
            Stmt::Semi(expr, semi)
        } else {
            Stmt::Expr(expr)
        }
    }

    fn fold_type(&mut self, ty: Type) -> Type {
        match ty {
            Type::Macro(ty_macro) => match ForTuplesMacro::try_from(&ty_macro.mac, false) {
                Ok(Some(for_tuples)) => Type::Verbatim(for_tuples.expand(
                    &self.tuple_placeholder_ident,
                    self.tuples,
                    false,
                )),
                Ok(None) => fold::fold_type_macro(self, ty_macro).into(),
                Err(e) => {
                    self.errors.push(e);
                    Type::Verbatim(Default::default())
                }
            },
            _ => fold::fold_type(self, ty),
        }
    }

    fn fold_impl_item_method(&mut self, mut impl_item_method: ImplItemMethod) -> ImplItemMethod {
        let has_self = impl_item_method
            .sig
            .inputs
            .first()
            .map(|a| match a {
                FnArg::Receiver(_) => true,
                _ => false,
            })
            .unwrap_or(false);

        impl_item_method.sig = fold::fold_signature(self, impl_item_method.sig);

        // Store the old value and set the current one
        let old_has_self_parameter = self.has_self_parameter;
        self.has_self_parameter = has_self;

        impl_item_method.block = fold::fold_block(self, impl_item_method.block);
        self.has_self_parameter = old_has_self_parameter;

        impl_item_method
    }
}

/// Extracts the tuple placeholder ident from the given trait implementation.
fn extract_tuple_placeholder_ident(trait_impl: &ItemImpl) -> Result<Ident> {
    if let Type::Path(ref type_path) = *trait_impl.self_ty {
        if let Some(ident) = type_path.path.get_ident() {
            return Ok(ident.clone());
        }
    }

    Err(Error::new(
        trait_impl.self_ty.span(),
        "Expected an `Ident` as tuple placeholder.",
    ))
}

/// Generate the semi-automatic tuple implementations for a given trait implementation and the given tuples.
pub fn semi_automatic_impl(
    trait_impl: ItemImpl,
    tuple_elements: Vec<Ident>,
    min: Option<usize>,
) -> Result<TokenStream> {
    let placeholder_ident = extract_tuple_placeholder_ident(&trait_impl)?;

    let mut res = TokenStream::new();

    (min.unwrap_or(0)..=tuple_elements.len()).try_for_each(|i| {
        res.extend(ToTupleImplementation::generate_implementation(
            &trait_impl,
            &placeholder_ident,
            &tuple_elements[..i],
        )?);
        Ok::<_, Error>(())
    })?;

    Ok(res)
}
