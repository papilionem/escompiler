// @expected-stdout-begin
// 10
// 20
// @expected-stdout-end
// nested eval var leak scope - inner eval var leaks to outer eval's scope
function test() {
    eval("eval('var inner = 10'); var outer = 20;");
    console.log(inner);
    console.log(outer);
}
test();
