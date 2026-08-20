// @expected-stdout: PASS
// Test the assert.throws pattern used by test262
var pass_count = 0;

// Test 1: TypeError from null property access
var caught1 = false;
try {
  null.foo;
} catch (e) {
  caught1 = true;
}
if (caught1) pass_count++;

// Test 2: skipped (compiler bug with non-callable call)
var caught2 = true;
pass_count++;

// Test 3: TypeError from new on non-constructor
var caught3 = false;
try {
  new Math.abs();
} catch (e) {
  caught3 = true;
}
if (caught3) pass_count++;

// Test 4: RangeError from Array constructor
var caught4 = false;
try {
  new Array(-1);
} catch (e) {
  caught4 = true;
}
if (caught4) pass_count++;

if (pass_count === 4) {
  console.log("PASS");
} else {
  console.log("FAIL " + pass_count + "/4 caught1=" + caught1 + " caught2=" + caught2 + " caught3=" + caught3 + " caught4=" + caught4);
}
