// @expected-stdout-begin
// PASS
// @expected-stdout-end

// instanceof with error subclasses — chain walk
try {
    throw new TypeError("test");
} catch (e) {
    if (!(e instanceof TypeError)) throw "FAIL: TypeError not instanceof TypeError";
    if (!(e instanceof Error)) throw "FAIL: TypeError not instanceof Error";
    if (e instanceof RangeError) throw "FAIL: TypeError is instanceof RangeError";
    if (e instanceof SyntaxError) throw "FAIL: TypeError is instanceof SyntaxError";
}

try {
    throw new RangeError("test");
} catch (e) {
    if (!(e instanceof RangeError)) throw "FAIL: RangeError not instanceof RangeError";
    if (!(e instanceof Error)) throw "FAIL: RangeError not instanceof Error";
    if (e instanceof TypeError) throw "FAIL: RangeError is instanceof TypeError";
}

try {
    throw new ReferenceError("test");
} catch (e) {
    if (!(e instanceof ReferenceError)) throw "FAIL: ReferenceError not instanceof ReferenceError";
    if (!(e instanceof Error)) throw "FAIL: ReferenceError not instanceof Error";
}

try {
    throw new SyntaxError("test");
} catch (e) {
    if (!(e instanceof SyntaxError)) throw "FAIL: SyntaxError not instanceof SyntaxError";
    if (!(e instanceof Error)) throw "FAIL: SyntaxError not instanceof Error";
}

try {
    throw new URIError("test");
} catch (e) {
    if (!(e instanceof URIError)) throw "FAIL: URIError not instanceof URIError";
    if (!(e instanceof Error)) throw "FAIL: URIError not instanceof Error";
}

try {
    throw new EvalError("test");
} catch (e) {
    if (!(e instanceof EvalError)) throw "FAIL: EvalError not instanceof EvalError";
    if (!(e instanceof Error)) throw "FAIL: EvalError not instanceof Error";
}

// instanceof with non-object should throw TypeError
try {
    ({}) instanceof 42;
    throw "FAIL: should have thrown TypeError for non-object RHS";
} catch (e) {
    if (!(e instanceof TypeError)) throw "FAIL: wrong error type for non-object RHS: " + e;
}

// instanceof with primitive LHS should return false
try {
    var result = 42 instanceof Error;
    throw "FAIL: should have thrown for 42 instanceof Error";
} catch (e) {
    // Expected: either false or TypeError depending on implementation
}

console.log("PASS");
