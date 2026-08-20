// @expected-stdout-begin
// 42
// 42
// true
// @expected-stdout-end

// Object.defineProperty with writable: false
let obj = {};
Object.defineProperty(obj, 'x', { value: 42, writable: false, enumerable: true, configurable: true });
console.log(obj.x);

// Sloppy mode assignment to non-writable should silently fail
obj.x = 99;
console.log(obj.x);

// getOwnPropertyDescriptor should return writable: false
let desc = Object.getOwnPropertyDescriptor(obj, 'x');
console.log(desc.writable === false);
