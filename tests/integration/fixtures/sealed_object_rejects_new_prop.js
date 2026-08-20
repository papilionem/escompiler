// @expected-stdout-begin
// existing: 1
// caught TypeError
// no new: undefined
// @expected-stdout-end
'use strict';
var obj = { x: 1 };
Object.seal(obj);
console.log('existing: ' + obj.x);
try {
  obj.y = 42;
} catch (e) {
  console.log('caught TypeError');
}
console.log('no new: ' + obj.y);
