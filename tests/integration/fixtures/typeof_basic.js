// @expected-stdout-begin
// number
// string
// boolean
// undefined
// object
// function
// object
// @expected-stdout-end
console.log(typeof 42);
console.log(typeof "hello");
console.log(typeof true);
console.log(typeof undefined);
console.log(typeof null);
console.log(typeof function() {});
console.log(typeof {});
