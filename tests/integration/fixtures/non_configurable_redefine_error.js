// @expected-stdout-begin
// value: 42
// caught redefine error
// still: 42
// @expected-stdout-end
var obj = {};
Object.defineProperty(obj, 'x', {
  value: 42,
  writable: false,
  enumerable: true,
  configurable: false
});
console.log('value: ' + obj.x);

// Attempting to redefine a non-configurable property to accessor should throw
try {
  Object.defineProperty(obj, 'x', {
    get: function() { return 99; }
  });
} catch (e) {
  console.log('caught redefine error');
}
console.log('still: ' + obj.x);
