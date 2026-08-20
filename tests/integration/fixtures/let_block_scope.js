// @expected-stdout-begin
// inner 20
// outer 10
// @expected-stdout-end
let x = 10;
{
    let x = 20;
    console.log("inner", x);
}
console.log("outer", x);
