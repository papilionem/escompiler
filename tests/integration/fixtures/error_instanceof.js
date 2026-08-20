// @expected-stdout-begin
// true
// true
// true
// true
// false
// @expected-stdout-end
try {
    throw new TypeError("test");
} catch (e) {
    console.log(e instanceof TypeError);
    console.log(e instanceof Error);
}

try {
    throw new RangeError("test");
} catch (e) {
    console.log(e instanceof RangeError);
    console.log(e instanceof Error);
    console.log(e instanceof TypeError);
}
