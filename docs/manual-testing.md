# Test a file transfer

Run these commands from a [built checkout](../README.md#build-from-source) in the same terminal. First open `./target/release/megawren`, check that the address in **Status** belongs to your test account, then quit with **q**.

## Round trip

Create two small files under `target/`. The random directory name also identifies the test folder on MEGA.

```sh
bin=./target/release/megawren
work=$(mktemp -d target/megawren-test.XXXXXXXX)
name=${work##*/}
mkdir -p "$work/$name/notes" "$work/download"
printf 'Harbor walk\n' > "$work/$name/route.txt"
printf 'High tide at noon\n' > "$work/$name/notes/tide.txt"

"$bin" put "$work/$name" /
"$bin" ls --json "/$name"
"$bin" get "/$name" "$work/download"
diff -r "$work/$name" "$work/download/$name"
```

The listing should contain `route.txt` and the `notes` directory. A successful `diff` prints nothing and exits with 0.

## Keep a local edit

Change the downloaded copy and request it again.

```sh
printf 'Local edit\n' > "$work/download/$name/route.txt"
"$bin" get "/$name/route.txt" "$work/download/$name"
echo $?
cat "$work/download/$name/route.txt"
```

The download should exit with 1 and `cat` should print `Local edit`. Repeat with explicit overwrite permission and compare the bytes again.

```sh
"$bin" get --overwrite "/$name/route.txt" "$work/download/$name"
diff -r "$work/$name" "$work/download/$name"
```

## Check the Rubbish Bin

Run `"$bin"`, focus the test folder on MEGA, and press **Delete**. Check the folder name in the confirmation. **Enter** on the default **Cancel** should leave it in place. Repeat and choose **Move to Rubbish Bin**.

To test recovery, press **2**, mark the folder with **Space**, press **Enter**, and choose **Restore to original folder**. Press **1** to find it in the drive again. Move it back to the bin when finished. Local test files remain in `$work`.
