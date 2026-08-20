// @expected-stdout-begin
// 1
// 99
// @expected-stdout-end
// eval modifies with-object property
var obj = { x: 1 };
console.log(obj.x);
with (obj) {
    eval("x = 99");
}
console.log(obj.x);
