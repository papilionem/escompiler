// ToPrimitive basic: object coercion in + and == operators
// @expected-stdout-begin
// [object Object]1
// 1[object Object]
// true
// 0
// @expected-stdout-end
var obj = {};
console.log(obj + 1);        // "[object Object]" + "1" = "[object Object]1"
console.log(1 + obj);        // "1" + "[object Object]" = "1[object Object]"
console.log([] == false);    // ToPrimitive([]) = "", ToNumber("") = 0, ToNumber(false) = 0, true
console.log(+[]);            // ToNumber([]) = ToNumber("") = 0
