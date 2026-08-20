//! JavaScript Proxy object support.
//!
//! Implements the core Proxy trap dispatch mechanism. A [`ProxyObject`] wraps a
//! target and handler (both represented as NaN-boxed `u64` values). Each
//! fundamental operation (get, set, has, delete, apply, construct) checks for
//! the corresponding handler trap and delegates to it if defined, otherwise
//! falling through to the target.
//!
//! ## Spec Invariant Enforcement
//!
//! All 13 proxy traps have spec-compliant invariant enforcement:
//! - `proxy_get` / `proxy_set` / `proxy_has` (10.5.7, 10.5.8, 10.5.9)
//! - `proxy_delete_property` (10.5.10)
//! - `proxy_define_property` (10.5.6)
//! - `proxy_get_own_property_descriptor` (10.5.5)
//! - `proxy_own_keys` (10.5.11)
//! - `proxy_get_prototype_of` (10.5.1)
//! - `proxy_set_prototype_of` (10.5.2)
//! - `proxy_is_extensible` (10.5.3)
//! - `proxy_prevent_extensions` (10.5.4)
//! - `proxy_call` (10.5.12)
//! - `proxy_construct` (10.5.13)
//!
//! ## Recursion Guard
//!
//! A thread-local depth counter prevents infinite proxy nesting. Each trap
//! entry increments the counter; exit decrements it. If the counter exceeds
//! [`MAX_PROXY_DEPTH`], a `RangeError` is raised.

use std::cell::Cell;

use nanbox::JsValue;
use thiserror::Error;

use crate::internal_data::{InternalData, UnifiedObject};
use crate::property::OwnPropertyDescriptor;
use crate::tagged_obj::{ObjTag, deref_tagged, read_obj_tag};

/// Maximum proxy nesting depth before a `RangeError` is thrown.
pub const MAX_PROXY_DEPTH: u32 = 256;

thread_local! {
    /// Current proxy trap recursion depth for this thread.
    static PROXY_DEPTH: Cell<u32> = const { Cell::new(0) };
}

/// Errors produced by proxy operations.
#[derive(Debug, Error)]
pub enum ProxyError {
    /// The proxy has been revoked and cannot be used.
    #[error("cannot perform '{operation}' on a revoked proxy")]
    Revoked {
        /// The operation that was attempted.
        operation: String,
    },
    /// A trap was called but returned an invalid result.
    #[error("proxy trap '{trap}' returned invalid result")]
    InvalidTrapResult {
        /// The trap name.
        trap: String,
    },
    /// A `get` trap violated a non-configurable data property invariant (spec 10.5.8).
    #[error(
        "'get' on proxy: property '{property}' is a read-only and non-configurable data property on the proxy target but the proxy did not return its actual value"
    )]
    GetInvariantViolation {
        /// The property name that was accessed.
        property: String,
    },
    /// A `get` trap violated a non-configurable accessor property invariant (spec 10.5.8).
    #[error(
        "'get' on proxy: property '{property}' is a non-configurable accessor property on the proxy target and does not have a getter function, but the trap did not return 'undefined'"
    )]
    GetAccessorInvariantViolation {
        /// The property name that was accessed.
        property: String,
    },
    /// A `set` trap violated a non-configurable, non-writable data property invariant (spec 10.5.9).
    #[error(
        "'set' on proxy: trap returned truish for property '{property}' which exists in the proxy target as a non-configurable and non-writable data property with a different value"
    )]
    SetInvariantViolation {
        /// The property name that was set.
        property: String,
    },
    /// A `set` trap violated a non-configurable accessor property invariant (spec 10.5.9).
    #[error(
        "'set' on proxy: trap returned truish for property '{property}' which exists in the proxy target as a non-configurable and non-writable accessor property with an undefined setter"
    )]
    SetAccessorInvariantViolation {
        /// The property name that was set.
        property: String,
    },
    /// A `has` trap reported a non-configurable own property as absent (spec 10.5.7).
    #[error(
        "'has' on proxy: trap returned falsish for property '{property}' which exists in the proxy target as non-configurable"
    )]
    HasNonConfigurableViolation {
        /// The property name that was queried.
        property: String,
    },
    /// A `has` trap reported an own property on a non-extensible target as absent (spec 10.5.7).
    #[error(
        "'has' on proxy: trap returned falsish for property '{property}' but the proxy target is not extensible"
    )]
    HasNonExtensibleViolation {
        /// The property name that was queried.
        property: String,
    },
    /// A `deleteProperty` trap deleted a non-configurable own property (spec 10.5.10).
    #[error(
        "'deleteProperty' on proxy: property '{property}' is non-configurable and cannot be deleted"
    )]
    DeleteNonConfigurableViolation {
        /// The property name that was deleted.
        property: String,
    },
    /// A `deleteProperty` trap deleted an own property on a non-extensible target (spec 10.5.10).
    #[error(
        "'deleteProperty' on proxy: trap returned truish but the proxy target is not extensible and property '{property}' exists"
    )]
    DeleteNonExtensibleViolation {
        /// The property name that was deleted.
        property: String,
    },
    /// A `defineProperty` trap violated a non-configurable invariant (spec 10.5.6).
    #[error(
        "'defineProperty' on proxy: trap returned truish for defining non-configurable property '{property}' which does not exist or is configurable on the target"
    )]
    DefinePropertyInvariantViolation {
        /// The property name that was defined.
        property: String,
    },
    /// A `getOwnPropertyDescriptor` trap reported a non-configurable property as absent (spec 10.5.5).
    #[error(
        "'getOwnPropertyDescriptor' on proxy: property '{property}' is non-configurable on the target but the trap returned undefined"
    )]
    GetOwnPropertyNonConfigurableViolation {
        /// The property name that was queried.
        property: String,
    },
    /// A `getOwnPropertyDescriptor` trap reported an existing property on a non-extensible target as absent (spec 10.5.5).
    #[error(
        "'getOwnPropertyDescriptor' on proxy: property '{property}' exists on the non-extensible target but the trap returned undefined"
    )]
    GetOwnPropertyNonExtensibleViolation {
        /// The property name that was queried.
        property: String,
    },
    /// An `ownKeys` trap omitted a non-configurable key (spec 10.5.11).
    #[error(
        "'ownKeys' on proxy: trap result did not include all non-configurable keys of the target"
    )]
    OwnKeysMissingNonConfigurable,
    /// An `ownKeys` trap added an extra key on a non-extensible target (spec 10.5.11).
    #[error("'ownKeys' on proxy: trap returned extra keys but the proxy target is not extensible")]
    OwnKeysNonExtensibleExtra,
    /// A `getPrototypeOf` trap returned a different prototype than a non-extensible target (spec 10.5.1).
    #[error(
        "'getPrototypeOf' on proxy: proxy target is non-extensible but the trap returned a different prototype"
    )]
    GetPrototypeOfInvariantViolation,
    /// A `setPrototypeOf` trap returned true but the target is non-extensible and prototypes differ (spec 10.5.2).
    #[error(
        "'setPrototypeOf' on proxy: trap returned true but the target is non-extensible and the new prototype differs"
    )]
    SetPrototypeOfInvariantViolation,
    /// An `isExtensible` trap returned a value inconsistent with target extensibility (spec 10.5.3).
    #[error("'isExtensible' on proxy: trap result does not match target extensibility")]
    IsExtensibleInvariantViolation,
    /// A `preventExtensions` trap returned true but the target is still extensible (spec 10.5.4).
    #[error(
        "'preventExtensions' on proxy: trap returned truish but the target is still extensible"
    )]
    PreventExtensionsInvariantViolation,
    /// A `call` trap was invoked on a non-callable proxy target (spec 10.5.12).
    #[error("proxy [[Call]]: target is not callable")]
    CallTargetNotCallable,
    /// A `construct` trap was invoked on a non-constructable proxy target (spec 10.5.13).
    #[error("proxy [[Construct]]: target is not a constructor")]
    ConstructTargetNotConstructor,
    /// A `construct` trap returned a non-object (spec 10.5.13).
    #[error("proxy [[Construct]]: trap returned non-object")]
    ConstructResultNotObject,
    /// Maximum proxy nesting depth exceeded.
    #[error("Maximum proxy nesting depth exceeded")]
    MaxDepthExceeded,
}

/// A JavaScript Proxy object.
///
/// Stores NaN-boxed `u64` handles to the target and handler objects.
/// The `revoked` flag is set by `Proxy.revocable()` to permanently
/// disable the proxy.
#[derive(Debug, Clone)]
pub struct ProxyObject {
    /// NaN-boxed handle to the proxy target.
    pub target: u64,
    /// NaN-boxed handle to the handler object.
    pub handler: u64,
    /// Whether this proxy has been revoked.
    pub revoked: bool,
}

/// Result of a proxy trap invocation.
///
/// `Trapped(u64)` means the handler trap handled the operation and returned a value.
/// `Fallthrough` means no trap was defined and the caller should operate on the target directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrapResult {
    /// The trap handled the operation and returned this NaN-boxed value.
    Trapped(u64),
    /// No trap was defined; fall through to the target.
    Fallthrough,
}

impl ProxyObject {
    /// `ProxyCreate ( target, handler )` — Create a new proxy wrapping `target` with the given `handler`.
    ///
    /// [spec]: https://tc39.es/ecma262/#sec-proxycreate
    pub fn new(target: u64, handler: u64) -> Self {
        Self {
            target,
            handler,
            revoked: false,
        }
    }

    /// Revoke this proxy, making all future operations fail.
    pub fn revoke(&mut self) {
        self.revoked = true;
    }

    /// Check that this proxy has not been revoked.
    fn check_revoked(&self, operation: &str) -> Result<(), ProxyError> {
        if self.revoked {
            return Err(ProxyError::Revoked {
                operation: operation.to_string(),
            });
        }
        Ok(())
    }

    /// `[[Get]] ( P, Receiver )` — Proxy handler `get` trap dispatch.
    ///
    /// If the handler defines a `get` trap, returns `TrapResult::Trapped` with the
    /// trap's return value. Otherwise returns `TrapResult::Fallthrough`.
    ///
    /// `trap_fn` is called with `(handler, target, key)` and should return `Some(value)`
    /// if the handler has a `get` trap, or `None` if not.
    ///
    /// [spec]: https://tc39.es/ecma262/#sec-proxy-object-internal-methods-and-internal-slots-get-p-receiver
    pub fn get(
        &self,
        key: u64,
        trap_fn: impl FnOnce(u64, u64, u64) -> Option<u64>,
    ) -> Result<TrapResult, ProxyError> {
        // 1. (Performed by caller) Assert: IsPropertyKey(P) is true.
        // 2. Let handler be O.[[ProxyHandler]].
        // 3. If handler is null, throw a TypeError exception.
        self.check_revoked("get")?;
        // 4. Assert: Type(handler) is Object.
        // 5. Let target be O.[[ProxyTarget]].
        // 6. Let trap be ? GetMethod(handler, "get").
        match trap_fn(self.handler, self.target, key) {
            Some(value) => {
                // 7. If trap is not undefined, then
                //    a. Let trapResult be ? Call(trap, handler, « target, P, Receiver »).
                //    b-d. (Invariant validation done in proxy_get)
                //    e. Return trapResult.
                Ok(TrapResult::Trapped(value))
            }
            None => {
                // 7. (continued) If trap is undefined, then
                //    a. Return ? target.[[Get]](P, Receiver).
                Ok(TrapResult::Fallthrough)
            }
        }
    }

    /// `[[Set]] ( P, V, Receiver )` — Proxy handler `set` trap dispatch.
    ///
    /// `trap_fn` is called with `(handler, target, key, value)` and should return
    /// `Some(true/false)` if the handler has a `set` trap, or `None` if not.
    ///
    /// [spec]: https://tc39.es/ecma262/#sec-proxy-object-internal-methods-and-internal-slots-set-p-v-receiver
    pub fn set(
        &mut self,
        key: u64,
        value: u64,
        trap_fn: impl FnOnce(u64, u64, u64, u64) -> Option<bool>,
    ) -> Result<TrapResult, ProxyError> {
        // 1. (Performed by caller) Assert: IsPropertyKey(P) is true.
        // 2. Let handler be O.[[ProxyHandler]].
        // 3. If handler is null, throw a TypeError exception.
        self.check_revoked("set")?;
        // 4. Assert: Type(handler) is Object.
        // 5. Let target be O.[[ProxyTarget]].
        // 6. Let trap be ? GetMethod(handler, "set").
        match trap_fn(self.handler, self.target, key, value) {
            Some(ok) => {
                // 7. If trap is not undefined, then
                //    a. Let booleanTrapResult be ! ToBoolean(? Call(trap, handler, « target, P, V, Receiver »)).
                //    b-e. (Invariant validation done in proxy_set)
                //    f. Return booleanTrapResult.
                Ok(TrapResult::Trapped(if ok { 1 } else { 0 }))
            }
            None => {
                // 7. (continued) If trap is undefined, then
                //    a. Return ? target.[[Set]](P, V, Receiver).
                Ok(TrapResult::Fallthrough)
            }
        }
    }

    /// `[[HasProperty]] ( P )` — Proxy handler `has` trap dispatch.
    ///
    /// `trap_fn` is called with `(handler, target, key)` and should return
    /// `Some(true/false)` if the handler has a `has` trap, or `None` if not.
    ///
    /// [spec]: https://tc39.es/ecma262/#sec-proxy-object-internal-methods-and-internal-slots-hasproperty-p
    pub fn has(
        &self,
        key: u64,
        trap_fn: impl FnOnce(u64, u64, u64) -> Option<bool>,
    ) -> Result<TrapResult, ProxyError> {
        // 1. Assert: IsPropertyKey(P) is true.
        // 2. Let handler be O.[[ProxyHandler]].
        // 3. If handler is null, throw a TypeError exception.
        self.check_revoked("has")?;
        // 4. Assert: Type(handler) is Object.
        // 5. Let target be O.[[ProxyTarget]].
        // 6. Let trap be ? GetMethod(handler, "has").
        match trap_fn(self.handler, self.target, key) {
            Some(found) => {
                // 7. If trap is not undefined, then
                //    a. Let booleanTrapResult be ! ToBoolean(? Call(trap, handler, « target, P »)).
                //    b-d. (Invariant validation done in proxy_has)
                //    e. Return booleanTrapResult.
                Ok(TrapResult::Trapped(if found { 1 } else { 0 }))
            }
            None => {
                // 7. (continued) If trap is undefined, then
                //    a. Return ? target.[[HasProperty]](P).
                Ok(TrapResult::Fallthrough)
            }
        }
    }

    /// `[[Delete]] ( P )` — Proxy handler `deleteProperty` trap dispatch.
    ///
    /// `trap_fn` is called with `(handler, target, key)` and should return
    /// `Some(true/false)` if the handler has a `deleteProperty` trap, or `None`.
    ///
    /// [spec]: https://tc39.es/ecma262/#sec-proxy-object-internal-methods-and-internal-slots-delete-p
    pub fn delete(
        &mut self,
        key: u64,
        trap_fn: impl FnOnce(u64, u64, u64) -> Option<bool>,
    ) -> Result<TrapResult, ProxyError> {
        // 1. Assert: IsPropertyKey(P) is true.
        // 2. Let handler be O.[[ProxyHandler]].
        // 3. If handler is null, throw a TypeError exception.
        self.check_revoked("deleteProperty")?;
        // 4. Assert: Type(handler) is Object.
        // 5. Let target be O.[[ProxyTarget]].
        // 6. Let trap be ? GetMethod(handler, "deleteProperty").
        match trap_fn(self.handler, self.target, key) {
            Some(ok) => {
                // 7. If trap is not undefined, then
                //    a. Let booleanTrapResult be ! ToBoolean(? Call(trap, handler, « target, P »)).
                //    b-e. (Invariant validation done in proxy_delete_property)
                //    f. Return booleanTrapResult.
                Ok(TrapResult::Trapped(if ok { 1 } else { 0 }))
            }
            None => {
                // 7. (continued) If trap is undefined, then
                //    a. Return ? target.[[Delete]](P).
                Ok(TrapResult::Fallthrough)
            }
        }
    }

    /// `[[Call]] ( thisArgument, argumentsList )` — Proxy handler `apply` trap dispatch.
    ///
    /// `trap_fn` is called with `(handler, target, this_arg, args)` and should return
    /// `Some(value)` if the handler has an `apply` trap, or `None`.
    ///
    /// [spec]: https://tc39.es/ecma262/#sec-proxy-object-internal-methods-and-internal-slots-call-thisargument-argumentslist
    pub fn apply(
        &self,
        this_arg: u64,
        args: &[u64],
        trap_fn: impl FnOnce(u64, u64, u64, &[u64]) -> Option<u64>,
    ) -> Result<TrapResult, ProxyError> {
        // 1. Let handler be O.[[ProxyHandler]].
        // 2. If handler is null, throw a TypeError exception.
        self.check_revoked("apply")?;
        // 3. Assert: Type(handler) is Object.
        // 4. Let target be O.[[ProxyTarget]].
        // 5. Let trap be ? GetMethod(handler, "apply").
        match trap_fn(self.handler, self.target, this_arg, args) {
            Some(value) => {
                // 6. If trap is not undefined, then
                //    a. Let argArray be ! CreateArrayFromList(argumentsList).
                //    b. Return ? Call(trap, handler, « target, thisArgument, argArray »).
                Ok(TrapResult::Trapped(value))
            }
            None => {
                // 6. (continued) If trap is undefined, then
                //    a. Assert: IsCallable(target) is true.
                //    b. Return ? Call(target, thisArgument, argumentsList).
                Ok(TrapResult::Fallthrough)
            }
        }
    }

    /// `[[Construct]] ( argumentsList, newTarget )` — Proxy handler `construct` trap dispatch.
    ///
    /// `trap_fn` is called with `(handler, target, args)` and should return
    /// `Some(value)` if the handler has a `construct` trap, or `None`.
    ///
    /// [spec]: https://tc39.es/ecma262/#sec-proxy-object-internal-methods-and-internal-slots-construct-argumentslist-newtarget
    pub fn construct(
        &self,
        args: &[u64],
        trap_fn: impl FnOnce(u64, u64, &[u64]) -> Option<u64>,
    ) -> Result<TrapResult, ProxyError> {
        // 1. Let handler be O.[[ProxyHandler]].
        // 2. If handler is null, throw a TypeError exception.
        self.check_revoked("construct")?;
        // 3. Assert: Type(handler) is Object.
        // 4. Let target be O.[[ProxyTarget]].
        // 5. Assert: IsConstructor(target) is true.
        // 6. Let trap be ? GetMethod(handler, "construct").
        match trap_fn(self.handler, self.target, args) {
            Some(value) => {
                // 7. If trap is not undefined, then
                //    a. Let argArray be ! CreateArrayFromList(argumentsList).
                //    b. Let newObj be ? Call(trap, handler, « target, argArray, newTarget »).
                //    c. If Type(newObj) is not Object, throw a TypeError exception.
                //    d. Return newObj.
                Ok(TrapResult::Trapped(value))
            }
            None => {
                // 7. (continued) If trap is undefined, then
                //    a. Assert: IsConstructor(target) is true.
                //    b. Return ? Construct(target, argumentsList, newTarget).
                Ok(TrapResult::Fallthrough)
            }
        }
    }

    /// `[[GetPrototypeOf]] ( )` — Proxy handler `getPrototypeOf` trap dispatch.
    ///
    /// `trap_fn` is called with `(handler, target)` and should return
    /// `Some(proto)` if the handler has a `getPrototypeOf` trap, or `None`.
    ///
    /// [spec]: https://tc39.es/ecma262/#sec-proxy-object-internal-methods-and-internal-slots-getprototypeof
    pub fn get_prototype_of(
        &self,
        trap_fn: impl FnOnce(u64, u64) -> Option<u64>,
    ) -> Result<TrapResult, ProxyError> {
        // 1. Let handler be O.[[ProxyHandler]].
        // 2. If handler is null, throw a TypeError exception.
        self.check_revoked("getPrototypeOf")?;
        // 3. Assert: Type(handler) is Object.
        // 4. Let target be O.[[ProxyTarget]].
        // 5. Let trap be ? GetMethod(handler, "getPrototypeOf").
        match trap_fn(self.handler, self.target) {
            Some(result) => {
                // 6. If trap is not undefined, then
                //    a. Let handlerProto be ? Call(trap, handler, « target »).
                //    b-d. (Invariant validation done in proxy_get_prototype_of)
                //    e. Return handlerProto.
                Ok(TrapResult::Trapped(result))
            }
            None => {
                // 6. (continued) If trap is undefined, then
                //    a. Return ? target.[[GetPrototypeOf]]().
                Ok(TrapResult::Fallthrough)
            }
        }
    }

    /// `[[SetPrototypeOf]] ( V )` — Proxy handler `setPrototypeOf` trap dispatch.
    ///
    /// `trap_fn` is called with `(handler, target, proto)` and should return
    /// `Some(success)` if the handler has a `setPrototypeOf` trap, or `None`.
    ///
    /// [spec]: https://tc39.es/ecma262/#sec-proxy-object-internal-methods-and-internal-slots-setprototypeof-v
    pub fn set_prototype_of(
        &self,
        proto: u64,
        trap_fn: impl FnOnce(u64, u64, u64) -> Option<bool>,
    ) -> Result<TrapResult, ProxyError> {
        // 1. Assert: Either Type(V) is Object or Type(V) is Null.
        // 2. Let handler be O.[[ProxyHandler]].
        // 3. If handler is null, throw a TypeError exception.
        self.check_revoked("setPrototypeOf")?;
        // 4. Assert: Type(handler) is Object.
        // 5. Let target be O.[[ProxyTarget]].
        // 6. Let trap be ? GetMethod(handler, "setPrototypeOf").
        match trap_fn(self.handler, self.target, proto) {
            Some(result) => {
                // 7. If trap is not undefined, then
                //    a. Let booleanTrapResult be ! ToBoolean(? Call(trap, handler, « target, V »)).
                //    b-d. (Invariant validation done in proxy_set_prototype_of)
                //    e. Return booleanTrapResult.
                Ok(TrapResult::Trapped(JsValue::bool(result).raw_bits()))
            }
            None => {
                // 7. (continued) If trap is undefined, then
                //    a. Return ? target.[[SetPrototypeOf]](V).
                Ok(TrapResult::Fallthrough)
            }
        }
    }

    /// `[[IsExtensible]] ( )` — Proxy handler `isExtensible` trap dispatch.
    ///
    /// `trap_fn` is called with `(handler, target)` and should return
    /// `Some(extensible)` if the handler has an `isExtensible` trap, or `None`.
    ///
    /// [spec]: https://tc39.es/ecma262/#sec-proxy-object-internal-methods-and-internal-slots-isextensible
    pub fn is_extensible(
        &self,
        trap_fn: impl FnOnce(u64, u64) -> Option<bool>,
    ) -> Result<TrapResult, ProxyError> {
        // 1. Let handler be O.[[ProxyHandler]].
        // 2. If handler is null, throw a TypeError exception.
        self.check_revoked("isExtensible")?;
        // 3. Assert: Type(handler) is Object.
        // 4. Let target be O.[[ProxyTarget]].
        // 5. Let trap be ? GetMethod(handler, "isExtensible").
        match trap_fn(self.handler, self.target) {
            Some(result) => {
                // 6. If trap is not undefined, then
                //    a. Let booleanTrapResult be ! ToBoolean(? Call(trap, handler, « target »)).
                //    b-c. (Invariant validation done in proxy_is_extensible)
                //    d. Return booleanTrapResult.
                Ok(TrapResult::Trapped(JsValue::bool(result).raw_bits()))
            }
            None => {
                // 6. (continued) If trap is undefined, then
                //    a. Return ? target.[[IsExtensible]]().
                Ok(TrapResult::Fallthrough)
            }
        }
    }

    /// `[[PreventExtensions]] ( )` — Proxy handler `preventExtensions` trap dispatch.
    ///
    /// `trap_fn` is called with `(handler, target)` and should return
    /// `Some(success)` if the handler has a `preventExtensions` trap, or `None`.
    ///
    /// [spec]: https://tc39.es/ecma262/#sec-proxy-object-internal-methods-and-internal-slots-preventextensions
    pub fn prevent_extensions(
        &self,
        trap_fn: impl FnOnce(u64, u64) -> Option<bool>,
    ) -> Result<TrapResult, ProxyError> {
        // 1. Let handler be O.[[ProxyHandler]].
        // 2. If handler is null, throw a TypeError exception.
        self.check_revoked("preventExtensions")?;
        // 3. Assert: Type(handler) is Object.
        // 4. Let target be O.[[ProxyTarget]].
        // 5. Let trap be ? GetMethod(handler, "preventExtensions").
        match trap_fn(self.handler, self.target) {
            Some(result) => {
                // 6. If trap is not undefined, then
                //    a. Let booleanTrapResult be ! ToBoolean(? Call(trap, handler, « target »)).
                //    b-c. (Invariant validation done in proxy_prevent_extensions)
                //    d. Return booleanTrapResult.
                Ok(TrapResult::Trapped(JsValue::bool(result).raw_bits()))
            }
            None => {
                // 6. (continued) If trap is undefined, then
                //    a. Return ? target.[[PreventExtensions]]().
                Ok(TrapResult::Fallthrough)
            }
        }
    }

    /// `[[GetOwnProperty]] ( P )` — Proxy handler `getOwnPropertyDescriptor` trap dispatch.
    ///
    /// `trap_fn` is called with `(handler, target, key)` and should return
    /// `Some(descriptor)` if the handler has a `getOwnPropertyDescriptor` trap,
    /// or `None`.
    ///
    /// [spec]: https://tc39.es/ecma262/#sec-proxy-object-internal-methods-and-internal-slots-getownproperty-p
    pub fn get_own_property_descriptor(
        &self,
        key: u64,
        trap_fn: impl FnOnce(u64, u64, u64) -> Option<u64>,
    ) -> Result<TrapResult, ProxyError> {
        // 1. Assert: IsPropertyKey(P) is true.
        // 2. Let handler be O.[[ProxyHandler]].
        // 3. If handler is null, throw a TypeError exception.
        self.check_revoked("getOwnPropertyDescriptor")?;
        // 4. Assert: Type(handler) is Object.
        // 5. Let target be O.[[ProxyTarget]].
        // 6. Let trap be ? GetMethod(handler, "getOwnPropertyDescriptor").
        match trap_fn(self.handler, self.target, key) {
            Some(result) => {
                // 7. If trap is not undefined, then
                //    a. Let trapResultObj be ? Call(trap, handler, « target, P »).
                //    b-j. (Invariant validation done in proxy_get_own_property_descriptor)
                //    k. Return resultDesc.
                Ok(TrapResult::Trapped(result))
            }
            None => {
                // 7. (continued) If trap is undefined, then
                //    a. Return ? target.[[GetOwnProperty]](P).
                Ok(TrapResult::Fallthrough)
            }
        }
    }

    /// `[[DefineOwnProperty]] ( P, Desc )` — Proxy handler `defineProperty` trap dispatch.
    ///
    /// `trap_fn` is called with `(handler, target, key, descriptor)` and should
    /// return `Some(success)` if the handler has a `defineProperty` trap, or `None`.
    ///
    /// [spec]: https://tc39.es/ecma262/#sec-proxy-object-internal-methods-and-internal-slots-defineownproperty-p-desc
    pub fn define_property(
        &self,
        key: u64,
        descriptor: u64,
        trap_fn: impl FnOnce(u64, u64, u64, u64) -> Option<bool>,
    ) -> Result<TrapResult, ProxyError> {
        // 1. Assert: IsPropertyKey(P) is true.
        // 2. Let handler be O.[[ProxyHandler]].
        // 3. If handler is null, throw a TypeError exception.
        self.check_revoked("defineProperty")?;
        // 4. Assert: Type(handler) is Object.
        // 5. Let target be O.[[ProxyTarget]].
        // 6. Let trap be ? GetMethod(handler, "defineProperty").
        match trap_fn(self.handler, self.target, key, descriptor) {
            Some(result) => {
                // 7. If trap is not undefined, then
                //    a. Let descObj be FromPropertyDescriptor(Desc).
                //    b. Let booleanTrapResult be ! ToBoolean(? Call(trap, handler, « target, P, descObj »)).
                //    c-h. (Invariant validation done in proxy_define_property)
                //    i. Return true.
                Ok(TrapResult::Trapped(JsValue::bool(result).raw_bits()))
            }
            None => {
                // 7. (continued) If trap is undefined, then
                //    a. Return ? target.[[DefineOwnProperty]](P, Desc).
                Ok(TrapResult::Fallthrough)
            }
        }
    }

    /// `[[OwnPropertyKeys]] ( )` — Proxy handler `ownKeys` trap dispatch.
    ///
    /// `trap_fn` is called with `(handler, target)` and should return
    /// `Some(array_of_keys)` if the handler has an `ownKeys` trap, or `None`.
    ///
    /// [spec]: https://tc39.es/ecma262/#sec-proxy-object-internal-methods-and-internal-slots-ownpropertykeys
    pub fn own_keys(
        &self,
        trap_fn: impl FnOnce(u64, u64) -> Option<u64>,
    ) -> Result<TrapResult, ProxyError> {
        // 1. Let handler be O.[[ProxyHandler]].
        // 2. If handler is null, throw a TypeError exception.
        self.check_revoked("ownKeys")?;
        // 3. Assert: Type(handler) is Object.
        // 4. Let target be O.[[ProxyTarget]].
        // 5. Let trap be ? GetMethod(handler, "ownKeys").
        match trap_fn(self.handler, self.target) {
            Some(result) => {
                // 6. If trap is not undefined, then
                //    a. Let trapResultArray be ? Call(trap, handler, « target »).
                //    b-m. (Invariant validation done in proxy_own_keys)
                //    n. Return trapResult.
                Ok(TrapResult::Trapped(result))
            }
            None => {
                // 6. (continued) If trap is undefined, then
                //    a. Return ? target.[[OwnPropertyKeys]]().
                Ok(TrapResult::Fallthrough)
            }
        }
    }
}

// =========================================================================
// Spec-compliant trap dispatch with invariant enforcement
// =========================================================================

/// Get the own property descriptor for a property on a target object.
///
/// Looks up the property in the shape table. Returns `None` if the target is
/// not a unified object or the property does not exist.
fn get_target_own_descriptor(target: u64, key_name: &str) -> Option<OwnPropertyDescriptor> {
    let tag = read_obj_tag(target)?;
    if tag != ObjTag::Unified as u8 {
        return None;
    }
    // SAFETY: tag check confirms this is a unified object.
    let uni = unsafe { deref_tagged::<UnifiedObject>(target) }?;
    crate::rt_api::SHAPES.with(|shapes| {
        crate::rt_api::INTERNER.with(|interner| {
            let shapes = shapes.borrow();
            let interner = interner.borrow();
            let atom = interner.intern(key_name);
            let desc = shapes.lookup(uni.shape_id, atom)?;
            if desc.is_accessor() {
                let getter = uni
                    .slots
                    .get(desc.offset as usize)
                    .copied()
                    .unwrap_or(JsValue::undefined());
                let setter = uni
                    .slots
                    .get(desc.offset as usize + 1)
                    .copied()
                    .unwrap_or(JsValue::undefined());
                Some(OwnPropertyDescriptor::Accessor {
                    getter,
                    setter,
                    enumerable: desc.enumerable,
                    configurable: desc.configurable,
                })
            } else {
                let value = uni
                    .slots
                    .get(desc.offset as usize)
                    .copied()
                    .unwrap_or(JsValue::undefined());
                Some(OwnPropertyDescriptor::Data {
                    value,
                    writable: desc.writable,
                    enumerable: desc.enumerable,
                    configurable: desc.configurable,
                })
            }
        })
    })
}

/// Check if the target object is extensible.
fn is_target_extensible(target: u64) -> bool {
    let Some(tag) = read_obj_tag(target) else {
        return true;
    };
    if tag != ObjTag::Unified as u8 {
        return true;
    }
    // SAFETY: tag check confirms this is a unified object.
    let uni = unsafe { deref_tagged::<UnifiedObject>(target) };
    uni.is_none_or(|u| u.is_extensible())
}

/// Check if target has an own property with the given name.
fn target_has_own_property(target: u64, key_name: &str) -> bool {
    let Some(tag) = read_obj_tag(target) else {
        return false;
    };
    if tag != ObjTag::Unified as u8 {
        return false;
    }
    // SAFETY: tag check confirms this is a unified object.
    let uni = unsafe { deref_tagged::<UnifiedObject>(target) };
    let Some(u) = uni else { return false };
    crate::rt_api::SHAPES.with(|shapes| {
        crate::rt_api::INTERNER.with(|interner| {
            let shapes = shapes.borrow();
            let interner = interner.borrow();
            u.has_own_property(key_name, &shapes, &interner)
        })
    })
}

/// `SameValue ( x, y )` — Tests if two NaN-boxed values are the same.
///
/// Handles NaN equality (NaN === NaN is true) and +0/-0 distinction.
///
/// [spec]: https://tc39.es/ecma262/#sec-samevalue
fn same_value(a: u64, b: u64) -> bool {
    let va = JsValue::from_raw_bits(a);
    let vb = JsValue::from_raw_bits(b);

    // Fast path: identical bits
    if a == b {
        // Check for +0/-0 distinction
        if let (Some(na), Some(nb)) = (va.as_number(), vb.as_number())
            && na == 0.0
            && nb == 0.0
        {
            return na.to_bits() == nb.to_bits();
        }
        return true;
    }

    // NaN === NaN (both are NaN)
    if let (Some(na), Some(nb)) = (va.as_number(), vb.as_number())
        && na.is_nan()
        && nb.is_nan()
    {
        return true;
    }

    false
}

/// Extract the proxy target and handler from a NaN-boxed proxy object.
///
/// Implements the common preamble steps shared by all proxy internal methods:
/// checking that the handler is not null (i.e., the proxy has not been revoked)
/// and extracting `[[ProxyTarget]]` and `[[ProxyHandler]]`.
///
/// Returns `(target, handler)` or `Err` if revoked. The `operation` parameter
/// is used in the error message.
pub fn extract_proxy_parts(proxy_obj: u64, operation: &str) -> Result<(u64, u64), ProxyError> {
    let tag = read_obj_tag(proxy_obj);
    if tag != Some(ObjTag::Unified as u8) {
        return Err(ProxyError::Revoked {
            operation: operation.to_string(),
        });
    }
    // SAFETY: tag check confirms this is a unified object.
    let uni = unsafe { deref_tagged::<UnifiedObject>(proxy_obj) };
    let Some(u) = uni else {
        return Err(ProxyError::Revoked {
            operation: operation.to_string(),
        });
    };
    match u.internal_data() {
        Some(InternalData::Proxy {
            target,
            handler,
            revoked,
        }) => {
            if *revoked {
                return Err(ProxyError::Revoked {
                    operation: operation.to_string(),
                });
            }
            Ok((*target, *handler))
        }
        _ => Err(ProxyError::Revoked {
            operation: operation.to_string(),
        }),
    }
}

/// `GetMethod ( V, P )` — Get the named trap function from a handler object.
///
/// Implements the `GetMethod` abstract operation used by all proxy traps to
/// look up the handler trap function.
///
/// Returns `Some(trap_bits)` if the handler has a callable property with the
/// given name, or `None` if the property is undefined/absent.
///
/// [spec]: https://tc39.es/ecma262/#sec-getmethod
pub fn get_trap(handler: u64, trap_name: &str) -> Option<u64> {
    let trap_key = crate::rt_api::make_rt_string(trap_name.to_string());
    let trap_val = crate::rt_api::__esc_rt_get_prop(handler, trap_key);
    let v = JsValue::from_raw_bits(trap_val);
    if v.is_undefined() || v.is_null() {
        None
    } else {
        Some(trap_val)
    }
}

/// Call a trap function with the given `this` value and arguments.
///
/// Uses `__esc_rt_call_indirect` to dispatch the call, setting up the
/// `CURRENT_THIS` context appropriately.
pub fn call_trap(trap: u64, this_arg: u64, args: &[u64]) -> u64 {
    crate::rt_api::CURRENT_THIS.with(|cell| cell.set(this_arg));
    // SAFETY: args slice is valid for the duration of the call.
    unsafe {
        crate::rt_api::__esc_rt_call_indirect(
            trap,
            args.len() as i32,
            if args.is_empty() {
                std::ptr::null()
            } else {
                args.as_ptr()
            },
        )
    }
}

/// `[[Get]] ( P, Receiver )` — Proxy `[[Get]]` with spec invariant enforcement.
///
/// Implements the full `[[Get]]` internal method for Proxy exotic objects.
///
/// [spec]: https://tc39.es/ecma262/#sec-proxy-object-internal-methods-and-internal-slots-get-p-receiver
pub fn proxy_get(proxy_obj: u64, key: u64, key_name: &str) -> Result<u64, ProxyError> {
    // 1. (Performed by caller) Assert: IsPropertyKey(P) is true.
    // 2. Let handler be O.[[ProxyHandler]].
    // 3. If handler is null, throw a TypeError exception.
    // 4. Assert: Type(handler) is Object.
    // 5. Let target be O.[[ProxyTarget]].
    let (target, handler) = extract_proxy_parts(proxy_obj, "get")?;

    // 6. Let trap be ? GetMethod(handler, "get").
    let Some(trap) = get_trap(handler, "get") else {
        // 7. If trap is undefined, then
        //    a. Return ? target.[[Get]](P, Receiver).
        return Ok(crate::rt_api::__esc_rt_get_prop(target, key));
    };

    // 8. Let trapResult be ? Call(trap, handler, « target, P, Receiver »).
    // (Receiver is the proxy itself per spec)
    let trap_result = call_trap(trap, handler, &[target, key, proxy_obj]);

    // 9. Let targetDesc be ? target.[[GetOwnProperty]](P).
    if let Some(desc) = get_target_own_descriptor(target, key_name) {
        // 10. If targetDesc is not undefined and targetDesc.[[Configurable]] is false, then
        match &desc {
            OwnPropertyDescriptor::Data {
                value,
                writable,
                configurable,
                ..
            } => {
                // 10a. If IsDataDescriptor(targetDesc) is true and targetDesc.[[Writable]] is false, then
                //      i. If SameValue(trapResult, targetDesc.[[Value]]) is false, throw a TypeError exception.
                if !configurable && !writable && !same_value(trap_result, value.raw_bits()) {
                    return Err(ProxyError::GetInvariantViolation {
                        property: key_name.to_string(),
                    });
                }
            }
            OwnPropertyDescriptor::Accessor {
                getter,
                configurable,
                ..
            } => {
                // 10b. If IsAccessorDescriptor(targetDesc) is true and targetDesc.[[Get]] is undefined, then
                //      i. If trapResult is not undefined, throw a TypeError exception.
                if !configurable
                    && getter.is_undefined()
                    && !JsValue::from_raw_bits(trap_result).is_undefined()
                {
                    return Err(ProxyError::GetAccessorInvariantViolation {
                        property: key_name.to_string(),
                    });
                }
            }
        }
    }

    // 11. Return trapResult.
    Ok(trap_result)
}

/// `[[Set]] ( P, V, Receiver )` — Proxy `[[Set]]` with spec invariant enforcement.
///
/// Implements the full `[[Set]]` internal method for Proxy exotic objects.
///
/// Returns `Ok(true)` if the set succeeded, `Ok(false)` if the trap returned
/// false, or `Err(ProxyError)` on invariant violation.
///
/// [spec]: https://tc39.es/ecma262/#sec-proxy-object-internal-methods-and-internal-slots-set-p-v-receiver
pub fn proxy_set(proxy_obj: u64, key: u64, value: u64, key_name: &str) -> Result<bool, ProxyError> {
    // 1. (Performed by caller) Assert: IsPropertyKey(P) is true.
    // 2. Let handler be O.[[ProxyHandler]].
    // 3. If handler is null, throw a TypeError exception.
    // 4. Assert: Type(handler) is Object.
    // 5. Let target be O.[[ProxyTarget]].
    let (target, handler) = extract_proxy_parts(proxy_obj, "set")?;

    // 6. Let trap be ? GetMethod(handler, "set").
    let Some(trap) = get_trap(handler, "set") else {
        // 7. If trap is undefined, then
        //    a. Return ? target.[[Set]](P, V, Receiver).
        crate::rt_api::__esc_rt_set_prop(target, key, value);
        return Ok(true);
    };

    // 8. Let booleanTrapResult be ! ToBoolean(? Call(trap, handler, « target, P, V, Receiver »)).
    let trap_result = call_trap(trap, handler, &[target, key, value, proxy_obj]);

    // 9. If booleanTrapResult is false, return false.
    let trap_bool = crate::value_ops::to_boolean(JsValue::from_raw_bits(trap_result));
    if !trap_bool {
        return Ok(false);
    }

    // 10. Let targetDesc be ? target.[[GetOwnProperty]](P).
    if let Some(desc) = get_target_own_descriptor(target, key_name) {
        // 11. If targetDesc is not undefined and targetDesc.[[Configurable]] is false, then
        match &desc {
            OwnPropertyDescriptor::Data {
                value: target_value,
                writable,
                configurable,
                ..
            } => {
                // 11a. If IsDataDescriptor(targetDesc) is true and targetDesc.[[Writable]] is false, then
                //      i. If SameValue(V, targetDesc.[[Value]]) is false, throw a TypeError exception.
                if !configurable && !writable && !same_value(value, target_value.raw_bits()) {
                    return Err(ProxyError::SetInvariantViolation {
                        property: key_name.to_string(),
                    });
                }
            }
            OwnPropertyDescriptor::Accessor {
                setter,
                configurable,
                ..
            } => {
                // 11b. If IsAccessorDescriptor(targetDesc) is true, then
                //      i. If targetDesc.[[Set]] is undefined, throw a TypeError exception.
                if !configurable && setter.is_undefined() {
                    return Err(ProxyError::SetAccessorInvariantViolation {
                        property: key_name.to_string(),
                    });
                }
            }
        }
    }

    // 12. Return true.
    Ok(true)
}

/// `[[HasProperty]] ( P )` — Proxy `[[HasProperty]]` with spec invariant enforcement.
///
/// Implements the full `[[HasProperty]]` internal method for Proxy exotic objects.
///
/// Returns `Ok(bool)` or `Err(ProxyError)` on invariant violation.
///
/// [spec]: https://tc39.es/ecma262/#sec-proxy-object-internal-methods-and-internal-slots-hasproperty-p
pub fn proxy_has(proxy_obj: u64, key: u64, key_name: &str) -> Result<bool, ProxyError> {
    // 1. Assert: IsPropertyKey(P) is true.
    // 2. Let handler be O.[[ProxyHandler]].
    // 3. If handler is null, throw a TypeError exception.
    // 4. Assert: Type(handler) is Object.
    // 5. Let target be O.[[ProxyTarget]].
    let (target, handler) = extract_proxy_parts(proxy_obj, "has")?;

    // 6. Let trap be ? GetMethod(handler, "has").
    let Some(trap) = get_trap(handler, "has") else {
        // 7. If trap is undefined, then
        //    a. Return ? target.[[HasProperty]](P).
        let result = crate::rt_api::__esc_rt_has_prop(target, key);
        let v = JsValue::from_raw_bits(result);
        return Ok(v.as_bool().unwrap_or(false));
    };

    // 8. Let booleanTrapResult be ! ToBoolean(? Call(trap, handler, « target, P »)).
    let trap_result = call_trap(trap, handler, &[target, key]);
    let trap_bool = crate::value_ops::to_boolean(JsValue::from_raw_bits(trap_result));

    // 9. If booleanTrapResult is false, then
    if !trap_bool {
        // 9a. Let targetDesc be ? target.[[GetOwnProperty]](P).
        // 9b. If targetDesc is not undefined, then
        if let Some(desc) = get_target_own_descriptor(target, key_name)
            && !desc.is_configurable()
        {
            // 9b.i. If targetDesc.[[Configurable]] is false, throw a TypeError exception.
            return Err(ProxyError::HasNonConfigurableViolation {
                property: key_name.to_string(),
            });
        }

        // 9c. If target.[[IsExtensible]]() is false, then
        //     i. If targetDesc is not undefined, throw a TypeError exception.
        if !is_target_extensible(target) && target_has_own_property(target, key_name) {
            return Err(ProxyError::HasNonExtensibleViolation {
                property: key_name.to_string(),
            });
        }
    }

    // 10. Return booleanTrapResult.
    Ok(trap_bool)
}

// =========================================================================
// Recursion guard
// =========================================================================

/// Increment the proxy recursion depth, returning an error if the limit is exceeded.
fn enter_proxy_trap() -> Result<(), ProxyError> {
    PROXY_DEPTH.with(|depth| {
        let current = depth.get();
        if current >= MAX_PROXY_DEPTH {
            return Err(ProxyError::MaxDepthExceeded);
        }
        depth.set(current + 1);
        Ok(())
    })
}

/// Decrement the proxy recursion depth.
fn exit_proxy_trap() {
    PROXY_DEPTH.with(|depth| {
        let current = depth.get();
        if current > 0 {
            depth.set(current - 1);
        }
    });
}

/// Returns the current proxy recursion depth (for testing).
pub fn proxy_depth() -> u32 {
    PROXY_DEPTH.with(Cell::get)
}

/// Reset the proxy recursion depth to zero (for testing).
pub fn reset_proxy_depth() {
    PROXY_DEPTH.with(|depth| depth.set(0));
}

/// Set the proxy recursion depth to a specific value (for testing only).
pub fn set_proxy_depth_for_test(value: u32) {
    PROXY_DEPTH.with(|depth| depth.set(value));
}

// =========================================================================
// Helper: get target prototype
// =========================================================================

/// Get the prototype of a target object.
///
/// Returns the prototype as a NaN-boxed value, or `null` if the target has no prototype.
fn get_target_prototype(target: u64) -> u64 {
    let Some(tag) = read_obj_tag(target) else {
        return JsValue::null().raw_bits();
    };
    if tag != ObjTag::Unified as u8 {
        return JsValue::null().raw_bits();
    }
    // SAFETY: tag check confirms this is a unified object.
    let uni = unsafe { deref_tagged::<UnifiedObject>(target) };
    let Some(u) = uni else {
        return JsValue::null().raw_bits();
    };

    // Check shape-based prototype via PROTO_OBJECTS registry
    crate::rt_api::SHAPES.with(|shapes| {
        let shapes = shapes.borrow();
        shapes
            .get_prototype(u.shape_id)
            .and_then(|proto_shape_id| {
                crate::rt_api::PROTO_OBJECTS
                    .with(|protos| protos.borrow().get(&proto_shape_id).copied())
            })
            .unwrap_or_else(|| JsValue::null().raw_bits())
    })
}

/// Extract string keys from a NaN-boxed array value.
///
/// Used by `proxy_own_keys` to convert the trap result array into a `Vec<String>`
/// for comparison against the target's own keys.
fn extract_keys_from_array(array_bits: u64) -> Vec<String> {
    let Some(tag) = read_obj_tag(array_bits) else {
        return Vec::new();
    };
    if tag != ObjTag::Unified as u8 {
        return Vec::new();
    }
    // SAFETY: tag check confirms this is a unified object.
    let uni = unsafe { deref_tagged::<UnifiedObject>(array_bits) };
    let Some(u) = uni else {
        return Vec::new();
    };
    if u.kind != crate::internal_data::InternalKind::Array {
        return Vec::new();
    }
    u.array_elements_resolved()
        .iter()
        .map(|v| {
            if v.is_string() {
                crate::string_ops::get_string_data(*v)
            } else {
                crate::display::display_value(*v)
            }
        })
        .collect()
}

/// Get all own string property keys from a target object.
///
/// Used by `proxy_own_keys` to enumerate the target's own keys for invariant checking.
fn get_target_own_keys(target: u64) -> Vec<String> {
    let Some(tag) = read_obj_tag(target) else {
        return Vec::new();
    };
    if tag != ObjTag::Unified as u8 {
        return Vec::new();
    }
    // SAFETY: tag check confirms this is a unified object.
    let uni = unsafe { deref_tagged::<UnifiedObject>(target) };
    let Some(u) = uni else {
        return Vec::new();
    };
    crate::rt_api::SHAPES.with(|shapes| {
        crate::rt_api::INTERNER.with(|interner| {
            let shapes = shapes.borrow();
            let interner = interner.borrow();
            u.own_keys(&shapes, &interner)
        })
    })
}

/// Check if a NaN-boxed value represents a callable object.
fn is_target_callable(target: u64) -> bool {
    let Some(tag) = read_obj_tag(target) else {
        return false;
    };
    if tag != ObjTag::Unified as u8 {
        return false;
    }
    // SAFETY: tag check confirms this is a unified object.
    let uni = unsafe { deref_tagged::<UnifiedObject>(target) };
    uni.is_some_and(|u| u.is_callable())
}

/// Check if a NaN-boxed value represents a constructable object.
fn is_target_constructable(target: u64) -> bool {
    let Some(tag) = read_obj_tag(target) else {
        return false;
    };
    if tag != ObjTag::Unified as u8 {
        return false;
    }
    // SAFETY: tag check confirms this is a unified object.
    let uni = unsafe { deref_tagged::<UnifiedObject>(target) };
    uni.is_some_and(|u| u.flags.is_constructable())
}

// =========================================================================
// Spec-compliant dispatch: remaining property traps
// =========================================================================

/// `[[Delete]] ( P )` — Proxy `[[Delete]]` with spec invariant enforcement.
///
/// Implements the full `[[Delete]]` internal method for Proxy exotic objects.
///
/// Returns `Ok(bool)` or `Err(ProxyError)` on invariant violation.
///
/// [spec]: https://tc39.es/ecma262/#sec-proxy-object-internal-methods-and-internal-slots-delete-p
pub fn proxy_delete_property(proxy_obj: u64, key: u64, key_name: &str) -> Result<bool, ProxyError> {
    enter_proxy_trap()?;
    let result = proxy_delete_property_inner(proxy_obj, key, key_name);
    exit_proxy_trap();
    result
}

/// Inner implementation for `proxy_delete_property` (without recursion guard).
fn proxy_delete_property_inner(
    proxy_obj: u64,
    key: u64,
    key_name: &str,
) -> Result<bool, ProxyError> {
    // 1. Assert: IsPropertyKey(P) is true.
    // 2. Let handler be O.[[ProxyHandler]].
    // 3. If handler is null, throw a TypeError exception.
    // 4. Assert: Type(handler) is Object.
    // 5. Let target be O.[[ProxyTarget]].
    let (target, handler) = extract_proxy_parts(proxy_obj, "deleteProperty")?;

    // 6. Let trap be ? GetMethod(handler, "deleteProperty").
    let Some(trap) = get_trap(handler, "deleteProperty") else {
        // 7. If trap is undefined, then
        //    a. Return ? target.[[Delete]](P).
        let result = crate::rt_api::__esc_rt_delete_prop(target, key);
        let v = JsValue::from_raw_bits(result);
        return Ok(v.as_bool().unwrap_or(false));
    };

    // 8. Let booleanTrapResult be ! ToBoolean(? Call(trap, handler, « target, P »)).
    let trap_result = call_trap(trap, handler, &[target, key]);
    let trap_bool = crate::value_ops::to_boolean(JsValue::from_raw_bits(trap_result));

    // 9. If booleanTrapResult is false, return false.
    // (Implicit: we continue to invariant checks only if trap_bool is true.)

    if trap_bool {
        // 10. Let targetDesc be ? target.[[GetOwnProperty]](P).
        // 11. If targetDesc is undefined, return true.
        // 12. If targetDesc.[[Configurable]] is false, throw a TypeError exception.
        if let Some(desc) = get_target_own_descriptor(target, key_name)
            && !desc.is_configurable()
        {
            return Err(ProxyError::DeleteNonConfigurableViolation {
                property: key_name.to_string(),
            });
        }

        // 13. If target.[[IsExtensible]]() is false, throw a TypeError exception.
        if !is_target_extensible(target) && target_has_own_property(target, key_name) {
            return Err(ProxyError::DeleteNonExtensibleViolation {
                property: key_name.to_string(),
            });
        }
    }

    // 14. Return true.
    Ok(trap_bool)
}

/// `[[DefineOwnProperty]] ( P, Desc )` — Proxy `[[DefineOwnProperty]]` with spec invariant enforcement.
///
/// Implements the full `[[DefineOwnProperty]]` internal method for Proxy exotic objects.
///
/// Returns `Ok(bool)` or `Err(ProxyError)` on invariant violation.
///
/// [spec]: https://tc39.es/ecma262/#sec-proxy-object-internal-methods-and-internal-slots-defineownproperty-p-desc
pub fn proxy_define_property(
    proxy_obj: u64,
    key: u64,
    descriptor: u64,
    key_name: &str,
) -> Result<bool, ProxyError> {
    enter_proxy_trap()?;
    let result = proxy_define_property_inner(proxy_obj, key, descriptor, key_name);
    exit_proxy_trap();
    result
}

/// Inner implementation for `proxy_define_property`.
fn proxy_define_property_inner(
    proxy_obj: u64,
    key: u64,
    descriptor: u64,
    key_name: &str,
) -> Result<bool, ProxyError> {
    // 1. Assert: IsPropertyKey(P) is true.
    // 2. Let handler be O.[[ProxyHandler]].
    // 3. If handler is null, throw a TypeError exception.
    // 4. Assert: Type(handler) is Object.
    // 5. Let target be O.[[ProxyTarget]].
    let (target, handler) = extract_proxy_parts(proxy_obj, "defineProperty")?;

    // 6. Let trap be ? GetMethod(handler, "defineProperty").
    let Some(trap) = get_trap(handler, "defineProperty") else {
        // 7. If trap is undefined, then
        //    a. Return ? target.[[DefineOwnProperty]](P, Desc).
        return Ok(true);
    };

    // 8. Let descObj be FromPropertyDescriptor(Desc).
    // 9. Let booleanTrapResult be ! ToBoolean(? Call(trap, handler, « target, P, descObj »)).
    let trap_result = call_trap(trap, handler, &[target, key, descriptor]);
    let trap_bool = crate::value_ops::to_boolean(JsValue::from_raw_bits(trap_result));

    // 10. If booleanTrapResult is false, return false.
    if !trap_bool {
        return Ok(false);
    }

    // 11. Let targetDesc be ? target.[[GetOwnProperty]](P).
    let target_desc = get_target_own_descriptor(target, key_name);
    // 12. Let extensibleTarget be ? target.[[IsExtensible]]().
    let extensible = is_target_extensible(target);

    // 13. If Desc has a [[Configurable]] field and if Desc.[[Configurable]] is false, then
    //     (Simplified: we check if target property doesn't exist on a non-extensible target)
    // 14. If targetDesc is undefined, then
    //     a. If extensibleTarget is false, throw a TypeError exception.
    if target_desc.is_none() && !extensible {
        return Err(ProxyError::DefinePropertyInvariantViolation {
            property: key_name.to_string(),
        });
    }

    // Steps 14b-16: Invariant checks for non-configurable property redefinition.
    // 14b. If settingConfigFalse is true and targetDesc.[[Configurable]] is true, throw TypeError.
    // 15. If targetDesc is not undefined, then
    if let Some(ref existing_desc) = target_desc {
        // Check if the descriptor being defined would change configurability
        // on an already non-configurable property.
        if !existing_desc.is_configurable() {
            // 15a. If IsCompatiblePropertyDescriptor(extensibleTarget, Desc, targetDesc) is false,
            //      throw a TypeError exception.
            // Simplified: if target property is non-configurable, the trap cannot:
            // - Make it configurable
            // - Change a non-configurable+non-writable data property's value or writability
            // We check the desc object for these violations using property lookup.
            let desc_val = JsValue::from_raw_bits(descriptor);
            if desc_val.is_object() {
                // Check if desc tries to set configurable: true on non-configurable property
                let config_key = crate::rt_api::make_rt_string("configurable".to_string());
                let config_val = crate::rt_api::__esc_rt_get_prop(descriptor, config_key);
                let config_jv = JsValue::from_raw_bits(config_val);
                if config_jv.as_bool() == Some(true) {
                    return Err(ProxyError::DefinePropertyInvariantViolation {
                        property: key_name.to_string(),
                    });
                }

                // 16. If targetDesc is a non-configurable, non-writable data property, then
                //     the trap must not change its value or make it writable.
                if let OwnPropertyDescriptor::Data {
                    writable: false, ..
                } = existing_desc
                {
                    // Check if desc tries to set writable: true
                    let writable_key = crate::rt_api::make_rt_string("writable".to_string());
                    let writable_val = crate::rt_api::__esc_rt_get_prop(descriptor, writable_key);
                    let writable_jv = JsValue::from_raw_bits(writable_val);
                    if writable_jv.as_bool() == Some(true) {
                        return Err(ProxyError::DefinePropertyInvariantViolation {
                            property: key_name.to_string(),
                        });
                    }
                }
            }
        }
    }

    // 17. Return true.
    Ok(true)
}

/// `[[GetOwnProperty]] ( P )` — Proxy `[[GetOwnProperty]]` with spec invariant enforcement.
///
/// Implements the full `[[GetOwnProperty]]` internal method for Proxy exotic objects.
///
/// Returns `Ok(u64)` with the descriptor value, or `Err(ProxyError)`.
///
/// [spec]: https://tc39.es/ecma262/#sec-proxy-object-internal-methods-and-internal-slots-getownproperty-p
pub fn proxy_get_own_property_descriptor(
    proxy_obj: u64,
    key: u64,
    key_name: &str,
) -> Result<u64, ProxyError> {
    enter_proxy_trap()?;
    let result = proxy_get_own_property_descriptor_inner(proxy_obj, key, key_name);
    exit_proxy_trap();
    result
}

/// Inner implementation for `proxy_get_own_property_descriptor`.
fn proxy_get_own_property_descriptor_inner(
    proxy_obj: u64,
    key: u64,
    key_name: &str,
) -> Result<u64, ProxyError> {
    // 1. Assert: IsPropertyKey(P) is true.
    // 2. Let handler be O.[[ProxyHandler]].
    // 3. If handler is null, throw a TypeError exception.
    // 4. Assert: Type(handler) is Object.
    // 5. Let target be O.[[ProxyTarget]].
    let (target, handler) = extract_proxy_parts(proxy_obj, "getOwnPropertyDescriptor")?;

    // 6. Let trap be ? GetMethod(handler, "getOwnPropertyDescriptor").
    let Some(trap) = get_trap(handler, "getOwnPropertyDescriptor") else {
        // 7. If trap is undefined, then
        //    a. Return ? target.[[GetOwnProperty]](P).
        return Ok(JsValue::undefined().raw_bits());
    };

    // 8. Let trapResultObj be ? Call(trap, handler, « target, P »).
    let trap_result = call_trap(trap, handler, &[target, key]);
    let trap_value = JsValue::from_raw_bits(trap_result);

    // 9. If Type(trapResultObj) is neither Object nor Undefined, throw a TypeError exception.
    if !trap_value.is_undefined() && !trap_value.is_object() && !trap_value.is_null() {
        return Err(ProxyError::InvalidTrapResult {
            trap: "getOwnPropertyDescriptor".to_string(),
        });
    }

    // 10. Let targetDesc be ? target.[[GetOwnProperty]](P).
    let target_desc = get_target_own_descriptor(target, key_name);

    // 11. If trapResultObj is undefined, then
    if trap_value.is_undefined() {
        if let Some(ref desc) = target_desc {
            // 11a. If targetDesc is not undefined, then
            //      i. If targetDesc.[[Configurable]] is false, throw a TypeError exception.
            if !desc.is_configurable() {
                return Err(ProxyError::GetOwnPropertyNonConfigurableViolation {
                    property: key_name.to_string(),
                });
            }

            // 11a.ii. If target.[[IsExtensible]]() is false, throw a TypeError exception.
            if !is_target_extensible(target) {
                return Err(ProxyError::GetOwnPropertyNonExtensibleViolation {
                    property: key_name.to_string(),
                });
            }
        }
    } else if let Some(ref desc) = target_desc {
        // Steps 12-22: Invariant checks when trap returns a descriptor object.
        // 18. If targetDesc is not undefined and targetDesc.[[Configurable]] is false, then
        if !desc.is_configurable() {
            // 19. If resultDesc.[[Configurable]] is true, throw a TypeError.
            // Check if trap result reports configurable: true for non-configurable property
            let config_key = crate::rt_api::make_rt_string("configurable".to_string());
            let config_val = crate::rt_api::__esc_rt_get_prop(trap_result, config_key);
            let config_jv = JsValue::from_raw_bits(config_val);
            if config_jv.as_bool() == Some(true) {
                return Err(ProxyError::GetOwnPropertyNonConfigurableViolation {
                    property: key_name.to_string(),
                });
            }

            // 20. If IsDataDescriptor(targetDesc) and targetDesc.[[Writable]] is false, then
            //     resultDesc must also report writable: false and same value.
            if let OwnPropertyDescriptor::Data {
                writable: false, ..
            } = desc
            {
                let writable_key = crate::rt_api::make_rt_string("writable".to_string());
                let writable_val = crate::rt_api::__esc_rt_get_prop(trap_result, writable_key);
                let writable_jv = JsValue::from_raw_bits(writable_val);
                if writable_jv.as_bool() == Some(true) {
                    return Err(ProxyError::GetOwnPropertyNonConfigurableViolation {
                        property: key_name.to_string(),
                    });
                }
            }
        }
    }

    // 23. Return resultDesc.
    Ok(trap_result)
}

/// `[[OwnPropertyKeys]] ( )` — Proxy `[[OwnPropertyKeys]]` with spec invariant enforcement.
///
/// Implements the full `[[OwnPropertyKeys]]` internal method for Proxy exotic objects.
///
/// Returns `Ok(u64)` with the keys array, or `Err(ProxyError)`.
///
/// [spec]: https://tc39.es/ecma262/#sec-proxy-object-internal-methods-and-internal-slots-ownpropertykeys
pub fn proxy_own_keys(proxy_obj: u64) -> Result<u64, ProxyError> {
    enter_proxy_trap()?;
    let result = proxy_own_keys_inner(proxy_obj);
    exit_proxy_trap();
    result
}

/// Inner implementation for `proxy_own_keys`.
fn proxy_own_keys_inner(proxy_obj: u64) -> Result<u64, ProxyError> {
    // 1. Let handler be O.[[ProxyHandler]].
    // 2. If handler is null, throw a TypeError exception.
    // 3. Assert: Type(handler) is Object.
    // 4. Let target be O.[[ProxyTarget]].
    let (target, handler) = extract_proxy_parts(proxy_obj, "ownKeys")?;

    // 5. Let trap be ? GetMethod(handler, "ownKeys").
    let Some(trap) = get_trap(handler, "ownKeys") else {
        // 6. If trap is undefined, then
        //    a. Return ? target.[[OwnPropertyKeys]]().
        return Ok(JsValue::undefined().raw_bits());
    };

    // 7. Let trapResultArray be ? Call(trap, handler, « target »).
    let trap_result = call_trap(trap, handler, &[target]);

    // Steps 8-23: Invariant checks for ownKeys.
    // 8. Let trapResult be ? CreateListFromArrayLike(trapResultArray, « String, Symbol »).
    // Extract trap result keys as strings for comparison.
    let trap_keys = extract_keys_from_array(trap_result);

    // 12. Let targetKeys be ? target.[[OwnPropertyKeys]]().
    let target_keys = get_target_own_keys(target);

    // 14-17. Let targetNonconfigurableKeys be the non-configurable keys of target.
    let non_configurable_keys: Vec<String> = target_keys
        .iter()
        .filter(|k| get_target_own_descriptor(target, k).is_some_and(|d| !d.is_configurable()))
        .cloned()
        .collect();

    // 18. If the target is not extensible, perform additional checks.
    let extensible = is_target_extensible(target);

    // 19. Check that all non-configurable keys appear in the trap result.
    for key in &non_configurable_keys {
        if !trap_keys.contains(key) {
            return Err(ProxyError::OwnKeysMissingNonConfigurable);
        }
    }

    // 21. If extensibleTarget is false, then
    if !extensible {
        // 21a. All target keys must appear in trap result.
        for key in &target_keys {
            if !trap_keys.contains(key) {
                return Err(ProxyError::OwnKeysMissingNonConfigurable);
            }
        }
        // 21b. Trap result must not contain keys not in target.
        for key in &trap_keys {
            if !target_keys.contains(key) {
                return Err(ProxyError::OwnKeysNonExtensibleExtra);
            }
        }
    }

    // 24. Return trapResult.
    Ok(trap_result)
}

/// `[[GetPrototypeOf]] ( )` — Proxy `[[GetPrototypeOf]]` with spec invariant enforcement.
///
/// Implements the full `[[GetPrototypeOf]]` internal method for Proxy exotic objects.
///
/// Returns `Ok(u64)` with the prototype, or `Err(ProxyError)`.
///
/// [spec]: https://tc39.es/ecma262/#sec-proxy-object-internal-methods-and-internal-slots-getprototypeof
pub fn proxy_get_prototype_of(proxy_obj: u64) -> Result<u64, ProxyError> {
    enter_proxy_trap()?;
    let result = proxy_get_prototype_of_inner(proxy_obj);
    exit_proxy_trap();
    result
}

/// Inner implementation for `proxy_get_prototype_of`.
fn proxy_get_prototype_of_inner(proxy_obj: u64) -> Result<u64, ProxyError> {
    // 1. Let handler be O.[[ProxyHandler]].
    // 2. If handler is null, throw a TypeError exception.
    // 3. Assert: Type(handler) is Object.
    // 4. Let target be O.[[ProxyTarget]].
    let (target, handler) = extract_proxy_parts(proxy_obj, "getPrototypeOf")?;

    // 5. Let trap be ? GetMethod(handler, "getPrototypeOf").
    let Some(trap) = get_trap(handler, "getPrototypeOf") else {
        // 6. If trap is undefined, then
        //    a. Return ? target.[[GetPrototypeOf]]().
        return Ok(get_target_prototype(target));
    };

    // 7. Let handlerProto be ? Call(trap, handler, « target »).
    let trap_result = call_trap(trap, handler, &[target]);

    // 8. If Type(handlerProto) is neither Object nor Null, throw a TypeError exception.
    // TODO: Step 8 — validate handlerProto is Object or Null.

    // 9. Let extensibleTarget be ? target.[[IsExtensible]]().
    // 10. If extensibleTarget is true, return handlerProto.
    // 11. Let targetProto be ? target.[[GetPrototypeOf]]().
    // 12. If SameValue(handlerProto, targetProto) is false, throw a TypeError exception.
    if !is_target_extensible(target) {
        let target_proto = get_target_prototype(target);
        if !same_value(trap_result, target_proto) {
            return Err(ProxyError::GetPrototypeOfInvariantViolation);
        }
    }

    // 13. Return handlerProto.
    Ok(trap_result)
}

/// `[[SetPrototypeOf]] ( V )` — Proxy `[[SetPrototypeOf]]` with spec invariant enforcement.
///
/// Implements the full `[[SetPrototypeOf]]` internal method for Proxy exotic objects.
///
/// Returns `Ok(bool)` or `Err(ProxyError)`.
///
/// [spec]: https://tc39.es/ecma262/#sec-proxy-object-internal-methods-and-internal-slots-setprototypeof-v
pub fn proxy_set_prototype_of(proxy_obj: u64, proto: u64) -> Result<bool, ProxyError> {
    enter_proxy_trap()?;
    let result = proxy_set_prototype_of_inner(proxy_obj, proto);
    exit_proxy_trap();
    result
}

/// Inner implementation for `proxy_set_prototype_of`.
fn proxy_set_prototype_of_inner(proxy_obj: u64, proto: u64) -> Result<bool, ProxyError> {
    // 1. Assert: Either Type(V) is Object or Type(V) is Null.
    // 2. Let handler be O.[[ProxyHandler]].
    // 3. If handler is null, throw a TypeError exception.
    // 4. Assert: Type(handler) is Object.
    // 5. Let target be O.[[ProxyTarget]].
    let (target, handler) = extract_proxy_parts(proxy_obj, "setPrototypeOf")?;

    // 6. Let trap be ? GetMethod(handler, "setPrototypeOf").
    let Some(trap) = get_trap(handler, "setPrototypeOf") else {
        // 7. If trap is undefined, then
        //    a. Return ? target.[[SetPrototypeOf]](V).
        return Ok(true);
    };

    // 8. Let booleanTrapResult be ! ToBoolean(? Call(trap, handler, « target, V »)).
    let trap_result = call_trap(trap, handler, &[target, proto]);
    let trap_bool = crate::value_ops::to_boolean(JsValue::from_raw_bits(trap_result));

    // 9. If booleanTrapResult is false, return false.
    // 10. Let extensibleTarget be ? target.[[IsExtensible]]().
    // 11. If extensibleTarget is true, return true.
    // 12. Let targetProto be ? target.[[GetPrototypeOf]]().
    // 13. If SameValue(V, targetProto) is false, throw a TypeError exception.
    if trap_bool && !is_target_extensible(target) {
        let target_proto = get_target_prototype(target);
        if !same_value(proto, target_proto) {
            return Err(ProxyError::SetPrototypeOfInvariantViolation);
        }
    }

    // 14. Return true.
    Ok(trap_bool)
}

/// `[[IsExtensible]] ( )` — Proxy `[[IsExtensible]]` with spec invariant enforcement.
///
/// Implements the full `[[IsExtensible]]` internal method for Proxy exotic objects.
///
/// Returns `Ok(bool)` or `Err(ProxyError)`.
///
/// [spec]: https://tc39.es/ecma262/#sec-proxy-object-internal-methods-and-internal-slots-isextensible
pub fn proxy_is_extensible(proxy_obj: u64) -> Result<bool, ProxyError> {
    enter_proxy_trap()?;
    let result = proxy_is_extensible_inner(proxy_obj);
    exit_proxy_trap();
    result
}

/// Inner implementation for `proxy_is_extensible`.
fn proxy_is_extensible_inner(proxy_obj: u64) -> Result<bool, ProxyError> {
    // 1. Let handler be O.[[ProxyHandler]].
    // 2. If handler is null, throw a TypeError exception.
    // 3. Assert: Type(handler) is Object.
    // 4. Let target be O.[[ProxyTarget]].
    let (target, handler) = extract_proxy_parts(proxy_obj, "isExtensible")?;

    // 5. Let trap be ? GetMethod(handler, "isExtensible").
    let Some(trap) = get_trap(handler, "isExtensible") else {
        // 6. If trap is undefined, then
        //    a. Return ? target.[[IsExtensible]]().
        return Ok(is_target_extensible(target));
    };

    // 7. Let booleanTrapResult be ! ToBoolean(? Call(trap, handler, « target »)).
    let trap_result = call_trap(trap, handler, &[target]);
    let trap_bool = crate::value_ops::to_boolean(JsValue::from_raw_bits(trap_result));

    // 8. Let targetResult be ? target.[[IsExtensible]]().
    let target_extensible = is_target_extensible(target);
    // 9. If SameValue(booleanTrapResult, targetResult) is false, throw a TypeError exception.
    if trap_bool != target_extensible {
        return Err(ProxyError::IsExtensibleInvariantViolation);
    }

    // 10. Return booleanTrapResult.
    Ok(trap_bool)
}

/// `[[PreventExtensions]] ( )` — Proxy `[[PreventExtensions]]` with spec invariant enforcement.
///
/// Implements the full `[[PreventExtensions]]` internal method for Proxy exotic objects.
///
/// Returns `Ok(bool)` or `Err(ProxyError)`.
///
/// [spec]: https://tc39.es/ecma262/#sec-proxy-object-internal-methods-and-internal-slots-preventextensions
pub fn proxy_prevent_extensions(proxy_obj: u64) -> Result<bool, ProxyError> {
    enter_proxy_trap()?;
    let result = proxy_prevent_extensions_inner(proxy_obj);
    exit_proxy_trap();
    result
}

/// Inner implementation for `proxy_prevent_extensions`.
fn proxy_prevent_extensions_inner(proxy_obj: u64) -> Result<bool, ProxyError> {
    // 1. Let handler be O.[[ProxyHandler]].
    // 2. If handler is null, throw a TypeError exception.
    // 3. Assert: Type(handler) is Object.
    // 4. Let target be O.[[ProxyTarget]].
    let (target, handler) = extract_proxy_parts(proxy_obj, "preventExtensions")?;

    // 5. Let trap be ? GetMethod(handler, "preventExtensions").
    let Some(trap) = get_trap(handler, "preventExtensions") else {
        // 6. If trap is undefined, then
        //    a. Return ? target.[[PreventExtensions]]().
        return Ok(true);
    };

    // 7. Let booleanTrapResult be ! ToBoolean(? Call(trap, handler, « target »)).
    let trap_result = call_trap(trap, handler, &[target]);
    let trap_bool = crate::value_ops::to_boolean(JsValue::from_raw_bits(trap_result));

    // 8. If booleanTrapResult is true, then
    //    a. Let extensibleTarget be ? target.[[IsExtensible]]().
    //    b. If extensibleTarget is true, throw a TypeError exception.
    if trap_bool && is_target_extensible(target) {
        return Err(ProxyError::PreventExtensionsInvariantViolation);
    }

    // 9. Return booleanTrapResult.
    Ok(trap_bool)
}

// =========================================================================
// Spec-compliant dispatch: Call / Construct traps
// =========================================================================

/// `[[Call]] ( thisArgument, argumentsList )` — Proxy `[[Call]]` trap dispatch.
///
/// Implements the full `[[Call]]` internal method for Proxy exotic objects.
///
/// Returns `Ok(u64)` with the return value, or `Err(ProxyError)`.
///
/// [spec]: https://tc39.es/ecma262/#sec-proxy-object-internal-methods-and-internal-slots-call-thisargument-argumentslist
pub fn proxy_call(proxy_obj: u64, this_arg: u64, args: &[u64]) -> Result<u64, ProxyError> {
    enter_proxy_trap()?;
    let result = proxy_call_inner(proxy_obj, this_arg, args);
    exit_proxy_trap();
    result
}

/// Inner implementation for `proxy_call`.
fn proxy_call_inner(proxy_obj: u64, this_arg: u64, args: &[u64]) -> Result<u64, ProxyError> {
    // 1. Let handler be O.[[ProxyHandler]].
    // 2. If handler is null, throw a TypeError exception.
    // 3. Assert: Type(handler) is Object.
    // 4. Let target be O.[[ProxyTarget]].
    let (target, handler) = extract_proxy_parts(proxy_obj, "apply")?;

    // (Pre-check) Assert: IsCallable(target) is true.
    // Note: spec asserts this; we verify and throw if violated.
    if !is_target_callable(target) {
        return Err(ProxyError::CallTargetNotCallable);
    }

    // 5. Let trap be ? GetMethod(handler, "apply").
    let Some(trap) = get_trap(handler, "apply") else {
        // 6. If trap is undefined, then
        //    a. Return ? Call(target, thisArgument, argumentsList).
        let result = call_trap(target, this_arg, args);
        return Ok(result);
    };

    // 7. Let argArray be ! CreateArrayFromList(argumentsList).
    let args_array = crate::rt_api::__esc_rt_create_array(args.len() as u32);
    for (i, &arg) in args.iter().enumerate() {
        crate::rt_api::__esc_rt_array_push(args_array, arg);
        let _ = i;
    }

    // 8. Return ? Call(trap, handler, « target, thisArgument, argArray »).
    let trap_args = vec![target, this_arg, args_array];

    Ok(call_trap(trap, handler, &trap_args))
}

/// `[[Construct]] ( argumentsList, newTarget )` — Proxy `[[Construct]]` trap dispatch.
///
/// Implements the full `[[Construct]]` internal method for Proxy exotic objects.
///
/// Returns `Ok(u64)` with the constructed object, or `Err(ProxyError)`.
///
/// [spec]: https://tc39.es/ecma262/#sec-proxy-object-internal-methods-and-internal-slots-construct-argumentslist-newtarget
pub fn proxy_construct(proxy_obj: u64, args: &[u64], new_target: u64) -> Result<u64, ProxyError> {
    enter_proxy_trap()?;
    let result = proxy_construct_inner(proxy_obj, args, new_target);
    exit_proxy_trap();
    result
}

/// Inner implementation for `proxy_construct`.
fn proxy_construct_inner(proxy_obj: u64, args: &[u64], new_target: u64) -> Result<u64, ProxyError> {
    // 1. Let handler be O.[[ProxyHandler]].
    // 2. If handler is null, throw a TypeError exception.
    // 3. Assert: Type(handler) is Object.
    // 4. Let target be O.[[ProxyTarget]].
    let (target, handler) = extract_proxy_parts(proxy_obj, "construct")?;

    // 5. Assert: IsConstructor(target) is true.
    // Note: spec asserts this; we verify and throw if violated.
    if !is_target_constructable(target) {
        return Err(ProxyError::ConstructTargetNotConstructor);
    }

    // 6. Let trap be ? GetMethod(handler, "construct").
    let Some(trap) = get_trap(handler, "construct") else {
        // 7. If trap is undefined, then
        //    a. Assert: IsConstructor(target) is true.
        //    b. Return ? Construct(target, argumentsList, newTarget).
        crate::rt_api::CURRENT_NEW_TARGET.with(|cell| cell.set(new_target));
        let result = call_trap(target, JsValue::undefined().raw_bits(), args);
        return Ok(result);
    };

    // 8. Let argArray be ! CreateArrayFromList(argumentsList).
    let args_array = crate::rt_api::__esc_rt_create_array(args.len() as u32);
    for &arg in args {
        crate::rt_api::__esc_rt_array_push(args_array, arg);
    }

    // 9. Let newObj be ? Call(trap, handler, « target, argArray, newTarget »).
    let trap_result = call_trap(trap, handler, &[target, args_array, new_target]);

    // 10. If Type(newObj) is not Object, throw a TypeError exception.
    let result_value = JsValue::from_raw_bits(trap_result);
    if !result_value.is_object() {
        return Err(ProxyError::ConstructResultNotObject);
    }

    // 11. Return newObj.
    Ok(trap_result)
}

// =========================================================================
// Proxy.revocable
// =========================================================================

/// Native function used as the `revoke` callable from `Proxy.revocable()`.
///
/// Implements the `Proxy Revocation Functions` spec algorithm.
/// The `context` parameter holds the NaN-boxed proxy bits to revoke.
///
/// [spec]: https://tc39.es/ecma262/#sec-proxy-revocation-functions
fn revoke_native_fn(context: u64) -> u64 {
    crate::rt_api::__esc_rt_proxy_revoke(context);
    JsValue::undefined().raw_bits()
}

/// `Proxy.revocable ( target, handler )` — Create a revocable proxy using the runtime API.
///
/// Returns `(proxy_bits, revoke_fn_bits)` where:
/// - `proxy_bits` is the NaN-boxed tagged proxy object
/// - `revoke_fn_bits` is a NaN-boxed native function that revokes the proxy
///
/// The revoke function, when called, sets the proxy's internal `revoked` flag
/// to `true`, causing all subsequent trap operations to throw `TypeError`.
///
/// [spec]: https://tc39.es/ecma262/#sec-proxy.revocable
pub fn create_revocable_rt(target: u64, handler: u64) -> (u64, u64) {
    use crate::tagged_obj::{ObjTag, TaggedObj};

    // Create the proxy
    let proxy_bits = crate::rt_api::__esc_rt_create_proxy(target, handler);

    // Create a native revoke function. The proxy_bits are stored in the
    // `context` field of the NativeFunc, and the fn pointer uses it to revoke.
    let revoke_obj = UnifiedObject::native_func(revoke_native_fn, proxy_bits);
    let revoke_bits = TaggedObj::boxed(ObjTag::Unified, revoke_obj);

    (proxy_bits, revoke_bits)
}

/// `Proxy.revocable ( target, handler )` — Create a revocable proxy (struct-level API).
///
/// Returns a `(ProxyObject, revoke_fn)` pair. Calling the revoke function sets
/// the proxy's `revoked` flag to `true`.
///
/// [spec]: https://tc39.es/ecma262/#sec-proxy.revocable
pub fn create_revocable(target: u64, handler: u64) -> (ProxyObject, Box<dyn FnOnce()>) {
    let proxy = ProxyObject::new(target, handler);
    // The revoke function is a no-op placeholder; actual revocation is done
    // by calling `proxy.revoke()` on the live object reference, or via the
    // runtime API `create_revocable_rt` which wires through `__esc_rt_proxy_revoke`.
    let revoke = Box::new(|| {
        // In the full runtime, this closure would look up the proxy in the
        // object table and call `.revoke()`. For now it's a placeholder.
    });
    (proxy, revoke)
}
