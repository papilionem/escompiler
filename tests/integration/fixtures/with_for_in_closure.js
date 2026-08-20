// @expected-stdout-begin
// x
// y
// @expected-stdout-end
var target = { x: 1, y: 2 };
var fns = [];
for (var key in target) {
    (function(k) {
        with (target) {
            fns.push(function() { return k; });
        }
    })(key);
}
console.log(fns[0]());
console.log(fns[1]());
