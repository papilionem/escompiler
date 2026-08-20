// Per-iteration let with multiple variables
// @expected-stdout-begin
// 10
// 11
// 12
// @expected-stdout-end
let funcs = [];
for (let i = 0, j = 10; i < 3; i++, j++) {
    funcs.push(function() { return j; });
}
console.log(funcs[0]());
console.log(funcs[1]());
console.log(funcs[2]());
