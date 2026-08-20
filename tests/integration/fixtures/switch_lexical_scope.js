// @expected-stdout-begin
// outside
// inside
// outside
// @expected-stdout-end

// Switch creates its own lexical scope for let/const declarations
let x = "outside";

switch (0) {
    default:
        let y = "inside";
        console.log(x);
        console.log(y);
}

// y should not be accessible here; x should still be 'outside'
console.log(x);
