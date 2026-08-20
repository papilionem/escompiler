// @expected-stdout-begin
// TypeError
// bad argument
// RangeError
// out of range
// ReferenceError
// not found
// SyntaxError
// bad syntax
// URIError
// bad URI
// EvalError
// eval issue
// Error
// generic error
// @expected-stdout-end
try {
    throw new TypeError("bad argument");
} catch (e) {
    console.log(e.name);
    console.log(e.message);
}

try {
    throw new RangeError("out of range");
} catch (e) {
    console.log(e.name);
    console.log(e.message);
}

try {
    throw new ReferenceError("not found");
} catch (e) {
    console.log(e.name);
    console.log(e.message);
}

try {
    throw new SyntaxError("bad syntax");
} catch (e) {
    console.log(e.name);
    console.log(e.message);
}

try {
    throw new URIError("bad URI");
} catch (e) {
    console.log(e.name);
    console.log(e.message);
}

try {
    throw new EvalError("eval issue");
} catch (e) {
    console.log(e.name);
    console.log(e.message);
}

try {
    throw new Error("generic error");
} catch (e) {
    console.log(e.name);
    console.log(e.message);
}
