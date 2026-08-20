// Per-iteration let bindings: closures capture distinct values per iteration
// @expected-stdout-begin
// 0
// 1
// 2
// @expected-stdout-end
let funcs = [];
for (let i = 0; i < 3; i++) {
    funcs.push(function() { return i; });
}
console.log(funcs[0]());
console.log(funcs[1]());
console.log(funcs[2]());
