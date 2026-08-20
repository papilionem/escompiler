// @expected-stdout: PASS
// Test that exceptions from runtime are properly caught by try/catch
var caught = false;
try {
  JSON.parse("invalid json{{{");
} catch (e) {
  caught = true;
}

var caught2 = false;
try {
  var x = null;
  x.foo;
} catch (e) {
  caught2 = true;
}

if (caught && caught2) {
  console.log("PASS");
} else {
  console.log("FAIL caught=" + caught + " caught2=" + caught2);
}
