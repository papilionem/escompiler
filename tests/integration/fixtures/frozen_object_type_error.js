// @expected-stdout-begin
// original: 1
// caught TypeError
// still: 1
// @expected-stdout-end
'use strict';
var obj = { x: 1 };
Object.freeze(obj);
console.log('original: ' + obj.x);
try {
  obj.x = 999;
} catch (e) {
  console.log('caught TypeError');
}
console.log('still: ' + obj.x);
