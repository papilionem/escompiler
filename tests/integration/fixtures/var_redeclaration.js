// @expected-stdout-begin
// 2
// 1
// 5
// @expected-stdout-end

// var x = 1; var x = 2; — x should be 2 (redeclaration allowed)
var x = 1;
var x = 2;
console.log(x);

// var x = 1; var x; — x should still be 1 (no-init doesn't overwrite)
var y = 1;
var y;
console.log(y);

// function f(x) { var x; return x; } — parameter keeps its value
function f(x) { var x; return x; }
console.log(f(5));
