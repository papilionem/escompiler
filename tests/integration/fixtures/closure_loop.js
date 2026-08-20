// @expected-stdout-begin
// 0
// 1
// 2
// @expected-stdout-end
let fns = [];
for (let i = 0; i < 3; i = i + 1) {
    fns[i] = function() { return i; };
}
console.log(fns[0]());
console.log(fns[1]());
console.log(fns[2]());
