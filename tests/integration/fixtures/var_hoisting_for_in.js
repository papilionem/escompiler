// @expected-stdout-begin
// b
// string
// @expected-stdout-end

// var i in for-in should hoist to function scope
for (var i in {a: 1, b: 2}) {}
console.log(i);
console.log(typeof i);
