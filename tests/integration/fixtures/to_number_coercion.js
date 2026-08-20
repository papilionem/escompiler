// ToNumber type coercion edge cases
// @expected-stdout-begin
// 0
// 0
// 0
// 31
// 15
// 10
// Infinity
// -Infinity
// 123
// 0
// NaN
// 1
// 0
// NaN
// NaN
// 42
// @expected-stdout-end
console.log(+"");                // ToNumber("") = 0
console.log(+" ");               // ToNumber(" ") = 0
console.log(+"\t\n");            // ToNumber("\t\n") = 0
console.log(+"0x1F");            // ToNumber("0x1F") = 31
console.log(+"0o17");            // ToNumber("0o17") = 15
console.log(+"0b1010");          // ToNumber("0b1010") = 10
console.log(+"Infinity");        // ToNumber("Infinity") = Infinity
console.log(+"-Infinity");       // ToNumber("-Infinity") = -Infinity
console.log(+"  123  ");         // Whitespace trimmed
console.log(+null);              // ToNumber(null) = 0
console.log(+undefined);         // ToNumber(undefined) = NaN
console.log(+true);              // ToNumber(true) = 1
console.log(+false);             // ToNumber(false) = 0
console.log(+"abc");             // Invalid string = NaN
console.log(+"+0x1F");           // Signed hex = NaN
console.log(+"42");              // Numeric string
