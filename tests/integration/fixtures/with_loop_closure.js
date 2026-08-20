// @expected-stdout-begin
// 0
// 1
// 2
// @expected-stdout-end
var obj = { offset: 0 };
var fns = [];
for (var i = 0; i < 3; i = i + 1) {
    (function(n) {
        with (obj) {
            fns.push(function() { return n + offset; });
        }
    })(i);
}
console.log(fns[0]());
console.log(fns[1]());
console.log(fns[2]());
