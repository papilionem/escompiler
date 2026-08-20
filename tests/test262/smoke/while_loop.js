/*---
description: while loop
esid: sec-while-statement
---*/
var sum = 0;
var i = 0;
while (i < 5) {
    sum = sum + i;
    i = i + 1;
}
assert.sameValue(sum, 10);
