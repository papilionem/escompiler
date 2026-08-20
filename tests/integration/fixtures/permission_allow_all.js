// @expected-stdout-begin
// hello
// 10
// @expected-stdout-end
// With --allow-all, all features should be permitted.
// This test verifies that normal code compiles fine (baseline for permission tests).
function greet(name) {
    return "hello";
}
console.log(greet("world"));
console.log(5 + 5);
