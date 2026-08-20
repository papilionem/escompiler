// @expected-stdout-begin
// 10
// 10
// @expected-stdout-end
let obj = { val: 10 };
console.log(obj.val);
// Prototype mutation should not break subsequent access
Object.setPrototypeOf(obj, { extra: 99 });
console.log(obj.val);
