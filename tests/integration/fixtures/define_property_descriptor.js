// @expected-stdout-begin
// 42
// false
// true
// false
// @expected-stdout-end

// Round-trip: defineProperty -> getOwnPropertyDescriptor
let obj = {};
Object.defineProperty(obj, 'x', {
  value: 42,
  writable: false,
  enumerable: true,
  configurable: false
});

let desc = Object.getOwnPropertyDescriptor(obj, 'x');
console.log(desc.value);
console.log(desc.writable);
console.log(desc.enumerable);
console.log(desc.configurable);
