// @expected-stdout-begin
// 42 equals 42
// pass
// @expected-stdout-end

// Functions can have properties assigned to them
function myFunc() {}
myFunc.check = function(actual, expected) {
    console.log(actual + " equals " + expected);
};
myFunc.check(42, 42);

// Simulates the test262 assert.sameValue pattern
function assert(condition) {
    if (!condition) {
        throw "assertion failed";
    }
}
assert.sameValue = function(actual, expected) {
    if (actual !== expected) {
        throw "not same value";
    }
};
assert.sameValue(typeof 42, "number");
console.log("pass");
