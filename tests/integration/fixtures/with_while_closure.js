// @expected-stdout-begin
// a0
// a1
// a2
// @expected-stdout-end
var obj = { prefix: "a" };
var fns = [];
var i = 0;
while (i < 3) {
    (function(n) {
        with (obj) {
            fns.push(function() { return prefix + n; });
        }
    })(i);
    i = i + 1;
}
console.log(fns[0]());
console.log(fns[1]());
console.log(fns[2]());
