// @expected-stdout-begin
// 10
// 5
// @expected-stdout-end
{
    var x = 10;
}
console.log(x);
for (var i = 0; i < 5; i++) {}
console.log(i);
