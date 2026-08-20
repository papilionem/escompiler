//! JavaScript Symbol primitive type.
//!
//! Each Symbol has a unique `u32` identifier and an optional description string.
//! Well-known symbols (Symbol.iterator, etc.) use pre-assigned IDs (< 100).
//! User-created symbols get monotonically increasing IDs starting at 100.
//!
//! ## Key Functions
//!
//! - [`create_symbol`] — allocate a new unique symbol ID
//! - [`symbol_for`] — `Symbol.for(key)` global registry lookup/create
//! - [`symbol_key_for`] — `Symbol.keyFor(sym)` reverse lookup
//! - [`symbol_description`] — get a symbol's description string
//! - [`symbol_to_string`] — format as `"Symbol(description)"`
//!
//! ## Spec References
//!
//! - Well-known Symbols: <https://tc39.es/ecma262/#sec-well-known-symbols> (Table 1, §6.1.5.1)
//! - Symbol constructor: <https://tc39.es/ecma262/#sec-symbol-description> (§20.4.1.1)
//! - Symbol.for: <https://tc39.es/ecma262/#sec-symbol.for> (§20.4.2.2)
//! - Symbol.keyFor: <https://tc39.es/ecma262/#sec-symbol.keyfor> (§20.4.2.8)
//! - Symbol.prototype.description: <https://tc39.es/ecma262/#sec-symbol.prototype.description> (§20.4.3.2)
//! - Symbol.prototype.toString: <https://tc39.es/ecma262/#sec-symbol.prototype.tostring> (§20.4.3.3)

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};

/// Next available symbol ID. Starts at 100 to reserve 0-99 for well-known symbols.
static NEXT_SYMBOL_ID: AtomicU32 = AtomicU32::new(100);

// ---------------------------------------------------------------------------
// Well-known Symbols (§6.1.5.1, Table 1)
//
// The spec defines these as unique, immutable symbol values created during
// realm initialization. We use pre-assigned u32 IDs (1..=7) rather than
// runtime allocation, since they are known at compile time (AOT-first).
// ---------------------------------------------------------------------------

/// Well-known symbol ID for `Symbol.iterator`.
///
/// `@@iterator` — A method that returns the default Iterator for an object.
///
/// [spec]: https://tc39.es/ecma262/#sec-well-known-symbols (Table 1)
pub const SYMBOL_ITERATOR: u32 = 1;

/// Well-known symbol ID for `Symbol.toPrimitive`.
///
/// `@@toPrimitive` — A method that converts an object to a primitive value.
///
/// [spec]: https://tc39.es/ecma262/#sec-well-known-symbols (Table 1)
pub const SYMBOL_TO_PRIMITIVE: u32 = 2;

/// Well-known symbol ID for `Symbol.hasInstance`.
///
/// `@@hasInstance` — A method that determines if a constructor recognizes
/// an object as one of its instances. Called by `instanceof`.
///
/// [spec]: https://tc39.es/ecma262/#sec-well-known-symbols (Table 1)
pub const SYMBOL_HAS_INSTANCE: u32 = 3;

/// Well-known symbol ID for `Symbol.toStringTag`.
///
/// `@@toStringTag` — A string-valued property used in the default
/// `Object.prototype.toString` description.
///
/// [spec]: https://tc39.es/ecma262/#sec-well-known-symbols (Table 1)
pub const SYMBOL_TO_STRING_TAG: u32 = 4;

/// Well-known symbol ID for `Symbol.asyncIterator`.
///
/// `@@asyncIterator` — A method that returns the default AsyncIterator
/// for an object. Called by `for-await-of`.
///
/// [spec]: https://tc39.es/ecma262/#sec-well-known-symbols (Table 1)
pub const SYMBOL_ASYNC_ITERATOR: u32 = 5;

/// Well-known symbol ID for `Symbol.species`.
///
/// `@@species` — A function-valued property that is the constructor function
/// used to create derived objects.
///
/// [spec]: https://tc39.es/ecma262/#sec-well-known-symbols (Table 1)
pub const SYMBOL_SPECIES: u32 = 6;

/// Well-known symbol ID for `Symbol.unscopables`.
///
/// `@@unscopables` — An object-valued property whose own and inherited
/// property names are excluded from `with` environment bindings.
///
/// [spec]: https://tc39.es/ecma262/#sec-well-known-symbols (Table 1)
pub const SYMBOL_UNSCOPABLES: u32 = 7;

thread_local! {
    /// Global symbol registry for `Symbol.for()` / `Symbol.keyFor()`.
    ///
    /// Maps registry key strings to their assigned symbol IDs.
    static SYMBOL_REGISTRY: RefCell<HashMap<String, u32>> = RefCell::new(HashMap::new());

    /// Description storage: symbol_id -> description string.
    ///
    /// Only symbols created with a description are stored here.
    static SYMBOL_DESCRIPTIONS: RefCell<HashMap<u32, String>> = RefCell::new(HashMap::new());
}

/// `Symbol ( [ description ] )`
///
/// Creates a new unique Symbol value. When called as a function (not with
/// `new`), returns a new primitive Symbol with the optional description.
///
/// [spec]: https://tc39.es/ecma262/#sec-symbol-description (§20.4.1.1)
///
/// # Spec Algorithm
///
/// 1. If NewTarget is not undefined, throw a TypeError exception.
/// 2. If description is undefined, let descString be undefined.
/// 3. Else, let descString be ? ToString(description).
/// 4. Return a new unique Symbol value whose [[Description]] value is descString.
///
/// Note: Step 1 (NewTarget check) is handled at the call site / codegen level,
/// since Symbol cannot be used with `new`. Steps 2-3 (ToString coercion) are
/// handled by the caller before invoking this function.
pub fn create_symbol(description: Option<&str>) -> u32 {
    // 4. Return a new unique Symbol value whose [[Description]] value is descString.
    let id = NEXT_SYMBOL_ID.fetch_add(1, Ordering::Relaxed);
    if let Some(desc) = description {
        // Store [[Description]] for later retrieval via symbol_description.
        SYMBOL_DESCRIPTIONS.with(|descs| {
            descs.borrow_mut().insert(id, desc.to_string());
        });
    }
    id
}

/// `Symbol.for ( key )`
///
/// Returns the Symbol from the GlobalSymbolRegistry whose [[Key]] matches
/// the given string key, or creates a new one if none exists.
///
/// [spec]: https://tc39.es/ecma262/#sec-symbol.for (§20.4.2.2)
///
/// # Spec Algorithm
///
/// 1. Let stringKey be ? ToString(key).
/// 2. For each element e of the GlobalSymbolRegistry List, do
///    a. If SameValue(e.[[Key]], stringKey) is true, return e.[[Symbol]].
/// 3. Assert: GlobalSymbolRegistry does not currently contain an entry for stringKey.
/// 4. Let newSymbol be a new unique Symbol value whose [[Description]] value is stringKey.
/// 5. Append the Record { [[Key]]: stringKey, [[Symbol]]: newSymbol } to the
///    GlobalSymbolRegistry List.
/// 6. Return newSymbol.
///
/// Note: Step 1 (ToString coercion) is handled by the caller.
pub fn symbol_for(key: &str) -> u32 {
    SYMBOL_REGISTRY.with(|reg| {
        let mut reg = reg.borrow_mut();
        // 2. For each element e of the GlobalSymbolRegistry List, do
        //    a. If SameValue(e.[[Key]], stringKey) is true, return e.[[Symbol]].
        if let Some(&id) = reg.get(key) {
            return id;
        }
        // 3. Assert: GlobalSymbolRegistry does not currently contain an entry for stringKey.
        // 4. Let newSymbol be a new unique Symbol value whose [[Description]] value is stringKey.
        let id = NEXT_SYMBOL_ID.fetch_add(1, Ordering::Relaxed);
        // 5. Append the Record { [[Key]]: stringKey, [[Symbol]]: newSymbol } to the
        //    GlobalSymbolRegistry List.
        reg.insert(key.to_string(), id);
        // Store the key as the [[Description]].
        SYMBOL_DESCRIPTIONS.with(|descs| {
            descs.borrow_mut().insert(id, key.to_string());
        });
        // 6. Return newSymbol.
        id
    })
}

/// `Symbol.keyFor ( sym )`
///
/// Returns the key string from the GlobalSymbolRegistry for the given Symbol,
/// or `undefined` if the symbol is not in the registry.
///
/// [spec]: https://tc39.es/ecma262/#sec-symbol.keyfor (§20.4.2.8)
///
/// # Spec Algorithm
///
/// 1. If sym is not a Symbol, throw a TypeError exception.
/// 2. Return KeyForSymbol(sym).
///
/// ## KeyForSymbol ( sym ) — §20.4.2.9
///
/// 1. For each element e of the GlobalSymbolRegistry List, do
///    a. If SameValue(e.[[Symbol]], sym) is true, return e.[[Key]].
/// 2. Assert: GlobalSymbolRegistry does not currently contain an entry for sym.
/// 3. Return undefined.
///
/// Note: Step 1 of Symbol.keyFor (type check) is handled by the caller.
/// This function returns `None` for undefined (step 3 of KeyForSymbol).
pub fn symbol_key_for(id: u32) -> Option<String> {
    // KeyForSymbol(sym):
    SYMBOL_REGISTRY.with(|reg| {
        let reg = reg.borrow();
        // 1. For each element e of the GlobalSymbolRegistry List, do
        for (key, &sym_id) in reg.iter() {
            //    a. If SameValue(e.[[Symbol]], sym) is true, return e.[[Key]].
            if sym_id == id {
                return Some(key.clone());
            }
        }
        // 2-3. Not found — return undefined (represented as None).
        None
    })
}

/// `get Symbol.prototype.description`
///
/// Returns the `[[Description]]` value of the Symbol, or `undefined` if
/// the symbol was created without a description.
///
/// [spec]: https://tc39.es/ecma262/#sec-symbol.prototype.description (§20.4.3.2)
///
/// # Spec Algorithm
///
/// 1. Let s be the **this** value.
/// 2. Let sym be ? ThisSymbolValue(s).
/// 3. Return sym.[[Description]].
///
/// Note: Steps 1-2 (this value validation) are handled by the caller.
/// This function returns the `[[Description]]` directly (step 3).
/// Returns `None` to represent `undefined` when no description was given.
pub fn symbol_description(id: u32) -> Option<String> {
    // 3. Return sym.[[Description]].
    // Check well-known symbols first (their descriptions are compile-time constants).
    let well_known = match id {
        SYMBOL_ITERATOR => Some("Symbol.iterator"),
        SYMBOL_TO_PRIMITIVE => Some("Symbol.toPrimitive"),
        SYMBOL_HAS_INSTANCE => Some("Symbol.hasInstance"),
        SYMBOL_TO_STRING_TAG => Some("Symbol.toStringTag"),
        SYMBOL_ASYNC_ITERATOR => Some("Symbol.asyncIterator"),
        SYMBOL_SPECIES => Some("Symbol.species"),
        SYMBOL_UNSCOPABLES => Some("Symbol.unscopables"),
        _ => None,
    };
    if let Some(desc) = well_known {
        return Some(desc.to_string());
    }
    // For user-created symbols, look up the stored [[Description]].
    SYMBOL_DESCRIPTIONS.with(|descs| descs.borrow().get(&id).cloned())
}

/// `Symbol.prototype.toString ( )`
///
/// Returns a string of the form `"Symbol(description)"`.
///
/// [spec]: https://tc39.es/ecma262/#sec-symbol.prototype.tostring (§20.4.3.3)
///
/// # Spec Algorithm
///
/// 1. Let sym be ? ThisSymbolValue(**this** value).
/// 2. Return SymbolDescriptiveString(sym).
///
/// ## SymbolDescriptiveString ( sym ) — §20.4.3.3.1
///
/// 1. Let desc be sym's [[Description]] value.
/// 2. If desc is undefined, set desc to the empty String.
/// 3. Assert: desc is a String.
/// 4. Return the string-concatenation of "Symbol(", desc, and ")".
///
/// Note: Step 1 of toString (ThisSymbolValue) is handled by the caller.
pub fn symbol_to_string(id: u32) -> String {
    // SymbolDescriptiveString(sym):
    // 1. Let desc be sym's [[Description]] value.
    // 2. If desc is undefined, set desc to the empty String.
    // 4. Return the string-concatenation of "Symbol(", desc, and ")".
    match symbol_description(id) {
        Some(desc) => format!("Symbol({desc})"),
        None => "Symbol()".to_string(),
    }
}
