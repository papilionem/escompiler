/*---
description: for loop
esid: sec-for-statement
---*/
var sum = 0;
for (var i = 0; i < 5; i++) {
    sum = sum + i;
}
assert.sameValue(sum, 10);
