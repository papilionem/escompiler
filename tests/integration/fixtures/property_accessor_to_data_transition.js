// @expected-stdout-begin
// 100
// data: 42
// @expected-stdout-end
var obj = {};
Object.defineProperty(obj, 'x', {
  get: function() { return 100; },
  enumerable: true,
  configurable: true
});
console.log(obj.x);

// Transition from accessor back to data property
Object.defineProperty(obj, 'x', {
  value: 42,
  writable: true,
  enumerable: true,
  configurable: true
});
console.log('data: ' + obj.x);
