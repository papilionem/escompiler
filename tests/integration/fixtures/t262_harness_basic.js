// Tests that the test262 harness + assert.sameValue works end-to-end.
// If this SIGILLs, then ALL test262 tests would SIGILL.
// @expected-exit-code: 0
function Test262Error(message) {
    this.message = message || "";
}
function assert(condition, message) {
    if (!condition) {
        throw new Test262Error("assert failed: " + (message || ""));
    }
}
assert.sameValue = function (actual, expected, message) {
    if (actual !== expected) {
        throw new Test262Error(
            "assert.sameValue failed: expected " + expected + " but got " + actual +
            (message ? " (" + message + ")" : "")
        );
    }
};

var x = 1;
assert.sameValue(x, 1);
var y = "hello";
assert.sameValue(y, "hello");
