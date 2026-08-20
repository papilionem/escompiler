// @expected-stdout-begin
// 42
// 100
// @expected-stdout-end
var obj = { val: 42 };
var escaped;
with (obj) {
    escaped = function() { return val; };
}
// Closure escapes the with scope but still reads val from obj
console.log(escaped());
// Modify obj.val and call again — closure should see the update
obj.val = 100;
console.log(escaped());
