// @expected-exit-code: 1
// TDZ: accessing a let variable before its declaration throws ReferenceError
console.log(x);
let x = 5;
