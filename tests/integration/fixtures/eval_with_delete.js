// @expected-stdout-begin
// true
// undefined
// @expected-stdout-end
// eval deletes property from with-object
var obj = { x: 10, y: 20 };
with (obj) {
    eval("delete x");
}
console.log(obj.x === undefined);
console.log(obj.x);
