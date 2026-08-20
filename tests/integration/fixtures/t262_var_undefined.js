// Tests that uninitialized var is undefined (test262 pattern).
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

var x;
assert.sameValue(x, undefined);
