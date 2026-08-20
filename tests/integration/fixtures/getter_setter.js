// @expected-stdout-begin
// 10
// 20
// @expected-stdout-end
let obj = { _x: 10, get x() { return this._x; }, set x(v) { this._x = v; } };
console.log(obj.x);
obj.x = 20;
console.log(obj.x);
