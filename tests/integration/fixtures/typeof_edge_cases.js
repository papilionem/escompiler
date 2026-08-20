// @expected-stdout-begin
// undefined
// number
// function
// object
// function
// @expected-stdout-end

// typeof on undeclared variable should return "undefined", not throw
console.log(typeof notDeclaredAnywhere);

// typeof on a declared variable
var x = 42;
console.log(typeof x);

// typeof on built-in callable
console.log(typeof parseInt);

// typeof on built-in namespace
console.log(typeof Math);

// typeof on user-declared function
function myFunc() {}
console.log(typeof myFunc);
