// Abstract equality (==) type coercion edge cases
// @expected-stdout-begin
// true
// true
// false
// false
// false
// false
// true
// true
// true
// true
// false
// false
// @expected-stdout-end
console.log(null == undefined);    // true per spec
console.log(undefined == null);    // symmetric
console.log(null == 0);            // false (null only == null/undefined)
console.log(null == "");           // false
console.log(null == false);        // false
console.log(undefined == 0);       // false
console.log(true == 1);            // true (ToNumber(true) = 1)
console.log(false == 0);           // true (ToNumber(false) = 0)
console.log("1" == 1);             // true (ToNumber("1") = 1)
console.log("" == 0);              // true (ToNumber("") = 0)
console.log(NaN == NaN);           // false (NaN is never equal)
console.log(undefined == false);   // false (undefined only == null)
