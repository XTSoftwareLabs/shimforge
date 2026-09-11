use proc_macro::TokenStream;
use proc_macro2::TokenStream as Tokens;
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::visit::Visit;
use syn::{Expr, Path, ReturnType, Token, Type, TypeBareFn, parse_quote};

mod signature;

#[proc_macro]
pub fn __mock(input: TokenStream) -> TokenStream {
    expand(syn::parse(input)).into()
}

#[proc_macro]
pub fn __check_signature(input: TokenStream) -> TokenStream {
    expand_check(syn::parse(input)).into()
}

#[proc_macro]
pub fn __replace_local(input: TokenStream) -> TokenStream {
    expand_replacement(syn::parse(input)).into()
}

struct Replacement {
    mock: Input,
    target: Expr,
}

impl Parse for Replacement {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let root = input.parse()?;
        input.parse::<Token![,]>()?;
        let session = input.parse()?;
        input.parse::<Token![,]>()?;
        let source = input.parse()?;
        input.parse::<Token![,]>()?;
        let target = input.parse()?;
        input.parse::<Token![,]>()?;
        let signature = input.parse()?;
        Ok(Self {
            mock: Input {
                root,
                session,
                source,
                signature,
            },
            target,
        })
    }
}

fn expand_replacement(input: syn::Result<Replacement>) -> Tokens {
    let Replacement { mock, target } = match input {
        Ok(input) => input,
        Err(error) => return error.into_compile_error(),
    };
    let Input {
        root,
        session,
        source,
        signature,
    } = mock;
    let abi = &signature.abi;
    let unsafety = &signature.unsafety;
    let types: Vec<_> = (0..signature.inputs.len())
        .map(|index| format_ident!("__Arg{index}"))
        .collect();
    let names: Vec<_> = (0..signature.inputs.len())
        .map(|index| format_ident!("__arg{index}"))
        .collect();
    quote! {
        {
            struct __Site;
            #[allow(clippy::too_many_arguments)]
            #abi fn __dispatch<__Marker, #(#types,)* __Return>(#(#names: #types),*) -> __Return {
                let address = __dispatch::<__Marker, #(#types,)* __Return> as *const () as usize;
                let target = #root::__private::route(address).expect("replacement is not active");
                #root::__invoke!(target, #abi fn(#(#types),*) -> __Return, (#(#names),*))
            }
            fn __install<#(#types,)* __Return>(
                session: &mut #root::Session,
                source: #unsafety #abi fn(#(#types),*) -> __Return,
                target: #unsafety #abi fn(#(#types),*) -> __Return,
            ) -> ::std::result::Result<(), #root::Error> {
                #root::__install_replacement!(session, source as *const (),
                    __dispatch::<__Site, #(#types,)* __Return> as *const (), target as *const ())
            }
            __install(#session, #source, #target)
        }
    }
}

fn expand_check(input: syn::Result<SignatureCheck>) -> Tokens {
    match input {
        Ok(check) => signature::check(
            source_item(&check.source, &check.value),
            &check.signature,
            &check.target,
        ),
        Err(error) => error.into_compile_error(),
    }
}

struct SignatureCheck {
    source: Expr,
    value: Expr,
    target: Expr,
    signature: TypeBareFn,
}

impl Parse for SignatureCheck {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let source = input.parse()?;
        input.parse::<Token![,]>()?;
        let value = input.parse()?;
        input.parse::<Token![,]>()?;
        let target = input.parse()?;
        input.parse::<Token![,]>()?;
        let signature = input.parse()?;
        Ok(Self {
            source,
            value,
            target,
            signature,
        })
    }
}

fn source_item<'a>(source: &'a Expr, value: &'a Expr) -> &'a Expr {
    match source {
        Expr::Path(_) => source,
        Expr::Paren(paren) => source_item(&paren.expr, value),
        Expr::Group(group) => source_item(&group.expr, value),
        _ => value,
    }
}

struct Input {
    root: Path,
    session: Expr,
    source: Expr,
    signature: TypeBareFn,
}

impl Parse for Input {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let root = input.parse()?;
        input.parse::<Token![,]>()?;
        let session = input.parse()?;
        input.parse::<Token![,]>()?;
        let source = input.parse()?;
        input.parse::<Token![,]>()?;
        let signature: TypeBareFn = input.parse()?;
        if signature.variadic.is_some() {
            return Err(syn::Error::new_spanned(
                signature,
                "mock! does not support variadic functions",
            ));
        }
        if input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
        }
        Ok(Self {
            root,
            session,
            source,
            signature,
        })
    }
}

#[derive(Default)]
struct Borrowed(bool);

impl<'ast> Visit<'ast> for Borrowed {
    fn visit_type_reference(&mut self, node: &'ast syn::TypeReference) {
        self.0 |= node.lifetime.as_ref().is_none_or(|lt| lt.ident != "static");
        syn::visit::visit_type_reference(self, node);
    }

    fn visit_lifetime(&mut self, node: &'ast syn::Lifetime) {
        self.0 |= node.ident != "static";
    }
}

fn expand(input: syn::Result<Input>) -> Tokens {
    match input {
        Ok(input) => generate(input),
        Err(error) => error.into_compile_error(),
    }
}

fn generate(input: Input) -> Tokens {
    let Input {
        root,
        session,
        source,
        signature,
    } = input;
    let args: Vec<_> = signature.inputs.iter().map(|arg| &arg.ty).collect();
    let saved_source = parse_quote!(__shimforge_original);
    let signature_check = signature::check(
        source_item(&source, &saved_source),
        &signature,
        &parse_quote!(__shimforge_call),
    );
    let names: Vec<_> = (0..args.len())
        .map(|index| format_ident!("__shimforge_arg_{index}"))
        .collect();
    let infer = args.iter().map(|_| quote!(_));
    let output: Type = match &signature.output {
        ReturnType::Default => parse_quote!(()),
        ReturnType::Type(_, ty) => *ty.clone(),
    };
    let mut borrowed = Borrowed::default();
    borrowed.visit_type(&output);
    let builder_output = if borrowed.0 {
        quote!(())
    } else {
        quote!(#output)
    };
    let constants = (!borrowed.0).then(|| {
        quote! {
            impl<__Output> __ShimforgeBuilder<__Output>
            where __Output: ::std::marker::Send + 'static + ::std::convert::Into<#output> {
                pub fn returns(self, value: __Output) -> ::std::result::Result<#root::Expectation, #root::Error>
                where __Output: ::std::clone::Clone {
                    self.returning(move |#(#names),*| {
                        let _ = (#(#names),*);
                        value.clone().into()
                    })
                }

                pub fn return_once(self, value: __Output) -> ::std::result::Result<#root::Expectation, #root::Error> {
                    self.returning_once(move |#(#names),*| {
                        let _ = (#(#names),*);
                        value.into()
                    })
                }

                pub fn returns_default(self) -> ::std::result::Result<#root::Expectation, #root::Error>
                where __Output: ::std::default::Default {
                    self.returning(|#(#names),*| {
                        let _ = (#(#names),*);
                        __Output::default().into()
                    })
                }
            }
        }
    });
    let binder = &signature.lifetimes;
    let unsafety = &signature.unsafety;
    let abi = &signature.abi;
    let mut callable = signature.clone();
    callable.unsafety = None;
    callable.lifetimes = None;
    let parameters = binder.as_ref().map(|binder| &binder.lifetimes);
    let generics = parameters.map(|parameters| quote!(<#parameters>));
    quote! {{
        struct __ShimforgeRule {
            meta: ::std::sync::Arc<#root::__private::Meta>,
            matcher: ::std::boxed::Box<dyn #binder ::std::ops::Fn(#(&#args),*) -> bool + ::std::marker::Send + ::std::marker::Sync + 'static>,
            action: ::std::sync::Mutex<__ShimforgeAction>,
        }

        enum __ShimforgeAction {
            Repeat(::std::boxed::Box<dyn #binder ::std::ops::FnMut(#(#args),*) -> #output + ::std::marker::Send + 'static>),
            Once(::std::option::Option<::std::boxed::Box<dyn #binder ::std::ops::FnOnce(#(#args),*) -> #output + ::std::marker::Send + 'static>>),
        }

        impl #root::__private::Rule for __ShimforgeRule {
            fn meta(&self) -> &::std::sync::Arc<#root::__private::Meta> { &self.meta }
        }

        ::std::thread_local! {
            static __SHIMFORGE_LOCAL: ::std::cell::RefCell<::std::option::Option<::std::sync::Arc<#root::__private::State<__ShimforgeRule>>>> = const { ::std::cell::RefCell::new(::std::option::Option::None) };
        }
        static __SHIMFORGE_GLOBAL: ::std::sync::Mutex<::std::option::Option<::std::sync::Arc<#root::__private::State<__ShimforgeRule>>>> = ::std::sync::Mutex::new(::std::option::Option::None);
        static __SHIMFORGE_GLOBAL_ACTIVE: ::std::sync::atomic::AtomicBool = ::std::sync::atomic::AtomicBool::new(false);

        #[allow(clippy::too_many_arguments)]
        #abi fn __shimforge_call #generics (#(#names: #args),*) -> #output {
            let state = __SHIMFORGE_LOCAL.try_with(|slot| slot.try_borrow().ok().and_then(|state| state.clone())).ok().flatten()
                .or_else(|| {
                    if __SHIMFORGE_GLOBAL_ACTIVE.load(::std::sync::atomic::Ordering::Acquire) {
                        #root::__private::lock(&__SHIMFORGE_GLOBAL).clone()
                    } else {
                        ::std::option::Option::None
                    }
                });
            let state = match state {
                ::std::option::Option::Some(state) => state,
                ::std::option::Option::None => {
                    let address = __shimforge_call as *const () as usize;
                    let target = #root::__private::route(address).expect("mock is not active");
                    return #root::__invoke!(target, #callable, (#(#names),*));
                }
            };
            let _call = state.enter();
            let rule = state.select(&|rule| (rule.matcher)(#(&#names),*));
            let mut action = #root::__private::lock(&rule.action);
            match &mut *action {
                __ShimforgeAction::Repeat(action) => action(#(#names),*),
                __ShimforgeAction::Once(action) =>
                    action.take().expect("one-use return was already used")(#(#names),*),
            }
        }

        struct __ShimforgeMock {
            state: ::std::sync::Arc<#root::__private::State<__ShimforgeRule>>,
        }

        impl __ShimforgeMock {
            pub fn expect(&self) -> __ShimforgeBuilder<#builder_output> {
                __ShimforgeBuilder {
                    state: self.state.clone(),
                    config: #root::__private::Config::default(),
                    matcher: ::std::boxed::Box::new(|#(#names),*| { let _ = (#(#names),*); true }),
                    output: ::std::marker::PhantomData,
                }
            }

            pub fn verify(&self) -> ::std::result::Result<(), #root::Error> { self.state.verify() }

            pub fn checkpoint(&self) -> ::std::result::Result<(), #root::Error> { self.state.checkpoint() }
        }

        struct __ShimforgeBuilder<__Output> {
            state: ::std::sync::Arc<#root::__private::State<__ShimforgeRule>>,
            config: #root::__private::Config,
            matcher: ::std::boxed::Box<dyn #binder ::std::ops::Fn(#(&#args),*) -> bool + ::std::marker::Send + ::std::marker::Sync + 'static>,
            output: ::std::marker::PhantomData<fn() -> __Output>,
        }

        impl<__Output> __ShimforgeBuilder<__Output> {
            pub fn with<__Matcher>(mut self, matcher: __Matcher) -> Self
            where __Matcher: #binder ::std::ops::Fn(#(&#args),*) -> bool + ::std::marker::Send + ::std::marker::Sync + 'static {
                self.matcher = ::std::boxed::Box::new(matcher);
                self
            }

            pub fn times(mut self, count: impl ::std::convert::Into<#root::CallCount>) -> Self {
                self.config = self.config.times(count);
                self
            }

            pub fn once(mut self) -> Self {
                self.config = self.config.once();
                self
            }

            pub fn in_sequence(mut self, sequence: &#root::Sequence) -> Self {
                self.config = self.config.in_sequence(sequence);
                self
            }

            pub fn returning<__Action>(self, action: __Action) -> ::std::result::Result<#root::Expectation, #root::Error>
            where __Action: #binder ::std::ops::FnMut(#(#args),*) -> #output + ::std::marker::Send + 'static {
                self.state.add(self.config, |meta| __ShimforgeRule {
                    meta,
                    matcher: self.matcher,
                    action: ::std::sync::Mutex::new(__ShimforgeAction::Repeat(::std::boxed::Box::new(action))),
                })
            }

            pub fn returning_once<__Action>(self, action: __Action) -> ::std::result::Result<#root::Expectation, #root::Error>
            where __Action: #binder ::std::ops::FnOnce(#(#args),*) -> #output + ::std::marker::Send + 'static {
                self.state.add(self.config.for_once()?, |meta| __ShimforgeRule {
                    meta,
                    matcher: self.matcher,
                    action: ::std::sync::Mutex::new(__ShimforgeAction::Once(::std::option::Option::Some(::std::boxed::Box::new(action)))),
                })
            }

            pub fn never(self) -> ::std::result::Result<#root::Expectation, #root::Error> {
                self.times(0usize).panics("forbidden mock call")
            }

            pub fn panics(self, message: impl ::std::convert::Into<::std::string::String>) -> ::std::result::Result<#root::Expectation, #root::Error> {
                let message = message.into();
                self.returning(move |#(#names),*| {
                    let _ = (#(#names),*);
                    ::std::panic!("{}", message)
                })
            }
        }

        #constants

        (|| -> ::std::result::Result<__ShimforgeMock, #root::Error> {
            let __shimforge_original = #source;
            #signature_check
            let __shimforge_source = __shimforge_original as #unsafety #abi fn(#(#infer),*) -> _;
            #[allow(clippy::type_complexity)]
            let __shimforge_target: #signature = __shimforge_call;
            fn __shimforge_checked<T>(source: T, _: T) -> T { source }
            let __shimforge_source = __shimforge_checked(__shimforge_source, __shimforge_target);
            let __shimforge_session = (#session).__borrow();
            let __shimforge_thread = __shimforge_session.__thread();
            let __shimforge_state = #root::__private::State::new(::std::stringify!(#source));
            let __shimforge_set = |slot: &mut ::std::option::Option<::std::sync::Arc<#root::__private::State<__ShimforgeRule>>>| {
                if slot.is_some() {
                    return ::std::result::Result::Err(#root::Error::Expectation("mock site is already active".into()));
                }
                *slot = ::std::option::Option::Some(__shimforge_state.clone());
                ::std::result::Result::Ok(())
            };
            if __shimforge_thread.is_some() {
                __SHIMFORGE_LOCAL.with(|slot| __shimforge_set(&mut slot.borrow_mut()))?;
            } else {
                __shimforge_set(&mut #root::__private::lock(&__SHIMFORGE_GLOBAL))?;
                __SHIMFORGE_GLOBAL_ACTIVE.store(true, ::std::sync::atomic::Ordering::Release);
            }
            let __shimforge_detach = ::std::boxed::Box::new(move || {
                let state = if __shimforge_thread.is_some() {
                    __SHIMFORGE_LOCAL.with(|slot| slot.borrow_mut().take())
                } else {
                    __SHIMFORGE_GLOBAL_ACTIVE.store(false, ::std::sync::atomic::Ordering::Release);
                    #root::__private::lock(&__SHIMFORGE_GLOBAL).take()
                };
                ::std::mem::drop(state);
            });
            #root::__install!(__shimforge_session, __shimforge_source as *const (), __shimforge_target as *const (), __shimforge_state.clone(), __shimforge_detach)?;
            ::std::result::Result::Ok(__ShimforgeMock { state: __shimforge_state })
        })()
    }}
}

#[cfg(test)]
mod tests;
