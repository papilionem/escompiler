/*---
description: if/else statement
esid: sec-if-statement
---*/
var result;
if (true) {
    result = "yes";
} else {
    result = "no";
}
assert.sameValue(result, "yes");
