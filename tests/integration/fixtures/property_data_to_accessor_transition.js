// @expected-stdout-begin
// 42
// getter called
// 100
// @expected-stdout-end
var obj = {};
Object.defineProperty(obj, 'x', {
  value: 42,
  writable: true,
  enumerable: true,
  configurable: true
});
console.log(obj.x);

// Transition from data to accessor property (requires configurable: true)
Object.defineProperty(obj, 'x', {
  get: function() { return 100; },
  enumerable: true,
  configurable: true
});
console.log('getter called');
console.log(obj.x);
