// @expected-stdout-begin
// TypeError
// something went wrong
// string
// RangeError
// invalid length
// ReferenceError
// x is not defined
// @expected-stdout-end
try {
    throw new TypeError("something went wrong");
} catch (e) {
    console.log(e.name);
    console.log(e.message);
    console.log(typeof e.stack);
}

try {
    throw new RangeError("invalid length");
} catch (e) {
    console.log(e.name);
    console.log(e.message);
}

try {
    x;
} catch (e) {
    console.log(e.name);
    console.log(e.message);
}
