// @expected-exit-code: 1
// TDZ: accessing a const variable before its declaration throws ReferenceError
console.log(y);
const y = 10;
