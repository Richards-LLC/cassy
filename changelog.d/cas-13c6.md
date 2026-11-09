### Captured exit-status checks in workspace commands

The workspace shell analyzer now recognizes variables assigned from `$?` as numeric inputs to `test -eq` comparisons, while retaining conservative handling for other unresolved operands.
