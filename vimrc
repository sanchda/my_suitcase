" === Global Settings ===
set modeline
set encoding=utf-8"
set ignorecase
set smartcase
set foldenable
set foldmethod=indent 
set t_Co=256
set backspace=indent,eol,start 
set incsearch
set lazyredraw
set showmatch
set showcmd
set hlsearch
set nu
set ruler
set autoread
set nocompatible
set nobackup
set noerrorbells
set novisualbell
set t_vb=
set background=dark
set laststatus=2
set updatetime=500
set tm=500
set mouse=a
let $MYVIMRC="$SUITCASE/vimrc"

" === What information in statusline?
set statusline+=%F

" === Color column stuff
"execute "set colorcolumn=" . join(range(81,465), ',')
highlight ColorColumn ctermbg=lightgrey

" === Syntax Highlighting & auto-indent ===
let python_highlight_all=1
syntax on
syntax enable
set ofu=syntaxcomplete#Complete
set antialias
filetype on
filetype plugin on 
filetype indent on
set autoindent
set smartindent

" === vim SLIME stuff ===
let g:slime_target = "tmux"
let g:slime_paste_file = "$HOME/.slime_paste"

" === vimwiki/markdown ===
let g:vimwiki_list = [{'path': '~/vimwiki/', 'syntax': 'markdown', 'ext': '.md'}]

" === Wildmenu ===
set wildmenu
set wildmode=longest,list,full
set wildignore=.svn,CVS,*.o,*.a,*.class,*.mo,*.la,*.so,*.obj,*.swp,*.jpg,*.png,*.xpm,*.gif


" === Other Settings ===
" Show < or > when characters are not displayed on the left or right.
set list
set list listchars=nbsp:¬,tab:>-,precedes:<,extends:>

" === Coding tweaks ===
fu Dave_style()
  set shiftwidth=2
  set expandtab
  set tabstop=2
  set softtabstop=2
endf

fu Corp_style()
  set shiftwidth=2
  set tabstop=2
endf

fu Bazel_style()
  set shiftwidth=4
  set expandtab
  set tabstop=4
  set softtabstop=4
endf
au BufRead,BufNewFile *.sh,*.js,*.html,*.css,*py,*pyw,*.c,*.h,*.cpp,*.hpp,*.rs call Dave_style()
au BufRead,BufNewFile *.cc,*.cxx,*.hh,*.cxx,Makefile* call Dave_style()
au BufRead,BufNewFile Makefile* set noexpandtab

" Bazel
au BufRead,BufNewFile *.bzl,*.bazel call Bazel_style()

" TEX settings
au BufRead,BufNewFile *.tex call Dave_style()

" K settings
au BufRead,BufNewFile *.k set filetype=k3
au BufRead,BufNewFile *.k call Dave_style()

" JL settings
au BufRead,BufNewFile *.jl set filetype=julia
au BufRead,BufNewFile *.jl call Dave_style()

" strace settings
au BufRead,BufNewFile *.strace set filetype=strace

" go settings
au BufRead,BufNewFile *.go call Corp_style()

" cmake settings
au BufRead,BufNewFile CMakeLists.txt call Dave_style()

" Use the below highlight group when displaying bad whitespace is desired.
highlight BadWhitespace ctermbg=red guibg=red
" Display tabs at the beginning of a line in Python mode as bad.
au BufRead,BufNewFile *.py,*.pyw match BadWhitespace /^\t\+/
au BufRead,BufNewFile *.k,*.py,*.pyw,*.c,*.h match BadWhitespace /\s\+$/
 
" Python: not needed, C: prevents insertion of '*' at the beginning of every line in a comment
au BufRead,BufNewFile *.c,*.h set formatoptions-=c formatoptions-=o formatoptions-=r

