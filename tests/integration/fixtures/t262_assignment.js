// Tests basic assignment with test262 harness.
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
x = 1;
assert.sameValue(x, 1);
x = x + 1;
assert.sameValue(x, 2);
