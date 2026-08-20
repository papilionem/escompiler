// @expected-stdout-begin
// 0
// 2
// 5
// @expected-stdout-end
function count() {
    console.log(arguments.length);
}
count();
count(1, 2);
count(1, 2, 3, 4, 5);
