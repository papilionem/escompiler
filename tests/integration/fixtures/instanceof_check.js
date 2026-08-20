// @expected-stdout-begin
// true
// true
// false
// true
// true
// false
// true
// true
// @expected-stdout-end

// TypeError instanceof checks
try {
    throw new TypeError("test");
} catch (e) {
    console.log(e instanceof TypeError);
    console.log(e instanceof Error);
    console.log(e instanceof RangeError);
}

// RangeError instanceof checks
try {
    throw new RangeError("test");
} catch (e) {
    console.log(e instanceof RangeError);
    console.log(e instanceof Error);
    console.log(e instanceof TypeError);
}

// URIError instanceof checks
try {
    throw new URIError("test");
} catch (e) {
    console.log(e instanceof URIError);
    console.log(e instanceof Error);
}
