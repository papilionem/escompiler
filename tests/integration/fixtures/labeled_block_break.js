// @expected-stdout: 1
// break label on a non-loop block should exit the block
let x = 0;
myBlock: {
    x = 1;
    break myBlock;
    x = 2;
}
console.log(x);
