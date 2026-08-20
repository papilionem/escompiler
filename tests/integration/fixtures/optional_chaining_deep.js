// @expected-stdout-begin
// 99
// undefined
// @expected-stdout-end
let obj = { a: { b: { c: 99 } } };
console.log(obj?.a?.b?.c);
console.log(obj?.x?.y?.z);
