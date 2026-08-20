//! JsFunction: closure environment + function pointer for compiled/native functions.

use nanbox::JsValue;

/// Native function signature: takes a slice of arguments, returns a JsValue.
pub type NativeFn = fn(&[JsValue]) -> JsValue;

/// The kind of function (affects `this` binding, `arguments`, etc.).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionKind {
    Normal,
    Arrow,
    Generator,
    Async,
    AsyncGenerator,
}

/// A JavaScript function object.
pub struct JsFunction {
    /// The function name (may be empty for anonymous functions).
    pub name: String,
    /// The kind of function.
    pub kind: FunctionKind,
    /// The number of formal parameters.
    pub param_count: u32,
    /// Closed-over variables from the enclosing scope.
    pub env: Option<Vec<JsValue>>,
    /// Native implementation for built-in functions.
    pub native: Option<NativeFn>,
}

impl JsFunction {
    /// Creates a new function with the given name, kind, and parameter count.
    pub fn new(name: String, kind: FunctionKind, param_count: u32) -> Self {
        Self {
            name,
            kind,
            param_count,
            env: None,
            native: None,
        }
    }

    /// Attaches a closure environment to this function.
    pub fn with_env(mut self, env: Vec<JsValue>) -> Self {
        self.env = Some(env);
        self
    }

    /// Attaches a native implementation to this function.
    pub fn with_native(mut self, f: NativeFn) -> Self {
        self.native = Some(f);
        self
    }
}

impl std::fmt::Debug for JsFunction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JsFunction")
            .field("name", &self.name)
            .field("kind", &self.kind)
            .field("param_count", &self.param_count)
            .field("has_env", &self.env.is_some())
            .field("has_native", &self.native.is_some())
            .finish()
    }
}
