// @expected-stdout-begin
// caught: not a function
// caught: null not a function
// @expected-stdout-end
try {
  var x = 42;
  x();
} catch (e) {
  console.log('caught: not a function');
}

try {
  var y = null;
  y();
} catch (e) {
  console.log('caught: null not a function');
}
