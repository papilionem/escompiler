// @expected-stdout-begin
// 10
// 11
// 12
// @expected-stdout-end
var obj = { base: 10 };
var arr = [];
for (var i = 0; i < 3; i = i + 1) {
    (function(idx) {
        with (obj) {
            arr.push(function() { return base + idx; });
        }
    })(i);
}
console.log(arr[0]());
console.log(arr[1]());
console.log(arr[2]());
