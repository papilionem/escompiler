// @expected-stdout-begin
// two
// three
// @expected-stdout-end
let x = 2;
switch (x) {
    case 1:
        console.log("one");
        break;
    case 2:
        console.log("two");
    case 3:
        console.log("three");
        break;
    default:
        console.log("other");
}
